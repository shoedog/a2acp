//! Attested-prefix harvest sanitization and durable audit records.
//!
//! Bridge-core stays byte-blind to marker syntax. This module consumes only the
//! typed wrapper status carried on `Update::Done`, validates it against the
//! completed bridge-visible body, commits raw+decision audit records, and returns
//! the effective body.

use crate::attestation::{
    AttestedPrefixV1, HarvestSanitizationMode, InvalidAttestationReason, NoAttestationReason,
    PrefixAttestationCapability, PrefixAttestationStatus, PrefixBoundaryScheme,
    ATTESTED_PREFIX_ISSUER_V1,
};
use crate::error::BridgeError;
use crate::ports::TurnContext;
use ring::digest;
use serde::{Deserialize, Serialize};

pub const HARVEST_SCHEMA_VERSION: u16 = 1;
pub const SUSPICIOUS_THRESHOLD_PERCENT: u8 = 90;
pub const HARVEST_AUDIT_FAILED_REASON: &str = "harvest_audit_persist_failed";

pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionBodyOrigin {
    ModelText,
    BridgeSyntheticStreamError,
    BridgeSyntheticMissingDone,
    BridgeSyntheticCancellation,
    BridgeSyntheticEmptyFinal,
    BridgeSyntheticTwinDeath,
    BridgeStopReasonWithoutText,
}

impl CompletionBodyOrigin {
    fn synthetic_reason(self) -> Option<NoAttestationReason> {
        match self {
            Self::ModelText => None,
            Self::BridgeSyntheticStreamError => {
                Some(NoAttestationReason::BridgeSyntheticStreamError)
            }
            Self::BridgeSyntheticMissingDone => {
                Some(NoAttestationReason::BridgeSyntheticMissingDone)
            }
            Self::BridgeSyntheticCancellation => {
                Some(NoAttestationReason::BridgeSyntheticCancellation)
            }
            Self::BridgeSyntheticEmptyFinal => Some(NoAttestationReason::BridgeSyntheticEmptyFinal),
            Self::BridgeSyntheticTwinDeath => Some(NoAttestationReason::BridgeSyntheticTwinDeath),
            Self::BridgeStopReasonWithoutText => {
                Some(NoAttestationReason::BridgeStopReasonWithoutText)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarvestDecision {
    KeptOff,
    KeptNoAttestation,
    KeptInvalidAttestation,
    KeptSuspiciousAttestation,
    KeptZeroPrefix,
    CutAttested,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarvestRawRecordV1 {
    pub schema_version: u16,
    pub audit_id: String,
    pub task_id: String,
    pub run_id: String,
    pub node_id: String,
    pub attempt_id: u32,
    pub turn_id: String,
    pub backend_id: String,
    pub producer_id: String,
    pub declared_capability: PrefixAttestationCapability,
    pub raw_body: String,
    pub raw_len_bytes: u64,
    pub raw_body_sha256: [u8; 32],
    pub prefix_attestation: PrefixAttestationStatus,
    pub provenance_sha256: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarvestSanitizationDecisionV1 {
    pub schema_version: u16,
    pub audit_id: String,
    pub mode: HarvestSanitizationMode,
    pub decision: HarvestDecision,
    pub reason: Option<String>,
    pub node_id: String,
    pub producer_id: String,
    pub issuer_id: Option<String>,
    pub raw_body_sha256: [u8; 32],
    pub effective_body_sha256: [u8; 32],
    pub raw_len_bytes: u64,
    pub effective_len_bytes: u64,
    pub cut_byte_offset: Option<u64>,
    pub provenance_sha256: [u8; 32],
    pub suspicious_threshold_percent: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarvestAuditBundleV1 {
    pub raw: HarvestRawRecordV1,
    pub decision: HarvestSanitizationDecisionV1,
    pub created_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HarvestAuditCommit {
    Inserted,
    AlreadyPresentIdentical,
}

#[derive(Debug, thiserror::Error)]
pub enum HarvestAuditStoreError {
    #[error("harvest audit persistence failed: {0}")]
    Persistence(BoxError),
    #[error("harvest audit integrity conflict for {audit_id}")]
    IntegrityConflict { audit_id: String },
    #[error("harvest audit encoding failed: {0}")]
    Encoding(String),
    #[error("harvest audit lookup failed: {0}")]
    Lookup(BoxError),
}

#[async_trait::async_trait]
pub trait HarvestAuditStore: Send + Sync {
    /// Whether this store actually retains raw+decision audit bundles.
    /// Null adapters may acknowledge commits for byte-compatible Off-mode
    /// legacy paths, but enabled sanitization must reject those contexts before
    /// any workflow output is released.
    fn retains_audit_records(&self) -> bool {
        true
    }

    async fn commit_bundle(
        &self,
        raw: HarvestRawRecordV1,
        decision: HarvestSanitizationDecisionV1,
    ) -> Result<HarvestAuditCommit, HarvestAuditStoreError>;

    async fn get_by_audit_id(
        &self,
        audit_id: &str,
    ) -> Result<Option<HarvestAuditBundleV1>, HarvestAuditStoreError>;

    async fn get_by_attempt_key(
        &self,
        run_id: &str,
        node_id: &str,
        attempt_id: u32,
        turn_id: &str,
    ) -> Result<Option<HarvestAuditBundleV1>, HarvestAuditStoreError>;

    /// List bundles for a task in creation/audit-id order. If `after_audit_id`
    /// is provided but is not present among that task's bundles, returns an
    /// empty list rather than replaying from the beginning.
    async fn list_by_task_id(
        &self,
        task_id: &str,
        after_audit_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<HarvestAuditBundleV1>, HarvestAuditStoreError>;
}

#[derive(Debug, Default)]
pub struct NoopHarvestAuditStore;

#[async_trait::async_trait]
impl HarvestAuditStore for NoopHarvestAuditStore {
    fn retains_audit_records(&self) -> bool {
        false
    }

    async fn commit_bundle(
        &self,
        _raw: HarvestRawRecordV1,
        _decision: HarvestSanitizationDecisionV1,
    ) -> Result<HarvestAuditCommit, HarvestAuditStoreError> {
        Ok(HarvestAuditCommit::Inserted)
    }

    async fn get_by_audit_id(
        &self,
        _audit_id: &str,
    ) -> Result<Option<HarvestAuditBundleV1>, HarvestAuditStoreError> {
        Ok(None)
    }

    async fn get_by_attempt_key(
        &self,
        _run_id: &str,
        _node_id: &str,
        _attempt_id: u32,
        _turn_id: &str,
    ) -> Result<Option<HarvestAuditBundleV1>, HarvestAuditStoreError> {
        Ok(None)
    }

    async fn list_by_task_id(
        &self,
        _task_id: &str,
        _after_audit_id: Option<&str>,
        _limit: u32,
    ) -> Result<Vec<HarvestAuditBundleV1>, HarvestAuditStoreError> {
        Ok(Vec::new())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CompletionCommitError {
    #[error("harvest audit persist failed for {audit_id}: {source}")]
    HarvestAuditPersistFailed {
        audit_id: String,
        source: HarvestAuditStoreError,
    },
}

impl CompletionCommitError {
    pub fn audit_id(&self) -> &str {
        match self {
            Self::HarvestAuditPersistFailed { audit_id, .. } => audit_id,
        }
    }

    pub fn reason_code(&self) -> &'static str {
        HARVEST_AUDIT_FAILED_REASON
    }
}

impl From<CompletionCommitError> for BridgeError {
    fn from(value: CompletionCommitError) -> Self {
        BridgeError::HarvestAuditPersistFailed {
            audit_id: value.audit_id().to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedHarvest {
    pub audit_id: String,
    pub effective_body: String,
    pub decision: HarvestSanitizationDecisionV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SanitizationOutcome {
    pub effective_body: String,
    pub status: PrefixAttestationStatus,
    pub decision: HarvestDecision,
    pub reason: Option<String>,
    pub cut_byte_offset: Option<u64>,
    pub issuer_id: Option<String>,
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let hash = digest::digest(&digest::SHA256, bytes);
    let mut out = [0_u8; 32];
    out.copy_from_slice(hash.as_ref());
    out
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn lp(bytes: &[u8], out: &mut Vec<u8>) -> Result<(), HarvestAuditStoreError> {
    let len = u32::try_from(bytes.len()).map_err(|_| {
        HarvestAuditStoreError::Encoding("audit id component exceeds u32 length".to_string())
    })?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

pub fn audit_id_for(
    run_id: &str,
    node_id: &str,
    attempt_id: u32,
    turn_id: &str,
) -> Result<String, HarvestAuditStoreError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"APC-AUDIT-ID-V1");
    lp(run_id.as_bytes(), &mut bytes)?;
    lp(node_id.as_bytes(), &mut bytes)?;
    bytes.extend_from_slice(&attempt_id.to_be_bytes());
    lp(turn_id.as_bytes(), &mut bytes)?;
    Ok(format!("apc1_{}", hex_lower(&sha256(&bytes))))
}

fn reason_for_no_attestation(reason: &NoAttestationReason) -> String {
    match reason {
        NoAttestationReason::BackendDeclaredIncapable => "backend_declared_incapable",
        NoAttestationReason::ProtocolDowngrade => "protocol_downgrade",
        NoAttestationReason::SanitizationNotRequested => "sanitization_not_requested",
        NoAttestationReason::TurnMissingDeliverableBoundary => "turn_missing_deliverable_boundary",
        NoAttestationReason::TurnEndedWithoutDeliverable => "turn_ended_without_deliverable",
        NoAttestationReason::MultipleCommitMarkers => "multiple_commit_markers",
        NoAttestationReason::BackendProtocolViolation => "backend_protocol_violation",
        NoAttestationReason::BridgeSyntheticStreamError => "bridge_synthetic_stream_error",
        NoAttestationReason::BridgeSyntheticMissingDone => "bridge_synthetic_missing_done",
        NoAttestationReason::BridgeSyntheticCancellation => "bridge_synthetic_cancellation",
        NoAttestationReason::BridgeSyntheticEmptyFinal => "bridge_synthetic_empty_final",
        NoAttestationReason::BridgeSyntheticTwinDeath => "bridge_synthetic_twin_death",
        NoAttestationReason::BridgeStopReasonWithoutText => "bridge_stop_reason_without_text",
    }
    .to_string()
}

fn reason_for_invalid(reason: &InvalidAttestationReason) -> String {
    match reason {
        InvalidAttestationReason::UnsupportedVersion => "unsupported_version",
        InvalidAttestationReason::MalformedMetadata => "malformed_metadata",
        InvalidAttestationReason::DuplicateControlMetadata => "duplicate_control_metadata",
        InvalidAttestationReason::BackendCapabilityMismatch => "backend_capability_mismatch",
        InvalidAttestationReason::UntrustedIssuer => "untrusted_issuer",
        InvalidAttestationReason::ProducerMismatch => "producer_mismatch",
        InvalidAttestationReason::TurnMismatch => "turn_mismatch",
        InvalidAttestationReason::NonceMismatch => "nonce_mismatch",
        InvalidAttestationReason::LengthMismatch => "length_mismatch",
        InvalidAttestationReason::DigestMismatch => "digest_mismatch",
        InvalidAttestationReason::OffsetOverflow => "offset_overflow",
        InvalidAttestationReason::OffsetOutOfBounds => "offset_out_of_bounds",
        InvalidAttestationReason::EmptyDeliverable => "empty_deliverable",
        InvalidAttestationReason::OffsetNotUtf8Boundary => "offset_not_utf8_boundary",
    }
    .to_string()
}

fn invalid_status(
    raw_body: &str,
    producer_id: &str,
    turn_id: &str,
    reason: InvalidAttestationReason,
) -> SanitizationOutcome {
    let status = PrefixAttestationStatus::rejected(producer_id, turn_id, reason.clone());
    SanitizationOutcome {
        effective_body: raw_body.to_string(),
        status,
        decision: HarvestDecision::KeptInvalidAttestation,
        reason: Some(reason_for_invalid(&reason)),
        cut_byte_offset: None,
        issuer_id: None,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn sanitize_harvest(
    mode: HarvestSanitizationMode,
    origin: CompletionBodyOrigin,
    capability: &PrefixAttestationCapability,
    expected_producer_id: &str,
    expected_turn_id: &str,
    raw_body: &str,
    status: PrefixAttestationStatus,
) -> SanitizationOutcome {
    if mode == HarvestSanitizationMode::Off {
        return SanitizationOutcome {
            effective_body: raw_body.to_string(),
            status,
            decision: HarvestDecision::KeptOff,
            reason: None,
            cut_byte_offset: None,
            issuer_id: None,
        };
    }

    if let Some(reason) = origin.synthetic_reason() {
        return SanitizationOutcome {
            effective_body: raw_body.to_string(),
            status: PrefixAttestationStatus::unavailable(
                expected_producer_id,
                expected_turn_id,
                reason.clone(),
            ),
            decision: HarvestDecision::KeptNoAttestation,
            reason: Some(reason_for_no_attestation(&reason)),
            cut_byte_offset: None,
            issuer_id: None,
        };
    }

    match status {
        PrefixAttestationStatus::UnavailableV1(no) => SanitizationOutcome {
            effective_body: raw_body.to_string(),
            status: PrefixAttestationStatus::UnavailableV1(no.clone()),
            decision: HarvestDecision::KeptNoAttestation,
            reason: Some(reason_for_no_attestation(&no.reason)),
            cut_byte_offset: None,
            issuer_id: None,
        },
        PrefixAttestationStatus::Rejected(rejected) => SanitizationOutcome {
            effective_body: raw_body.to_string(),
            status: PrefixAttestationStatus::Rejected(rejected.clone()),
            decision: HarvestDecision::KeptInvalidAttestation,
            reason: Some(reason_for_invalid(&rejected.reason)),
            cut_byte_offset: None,
            issuer_id: None,
        },
        PrefixAttestationStatus::AttestedV1(attested) => validate_attested(
            capability,
            expected_producer_id,
            expected_turn_id,
            raw_body,
            attested,
        ),
    }
}

fn validate_attested(
    capability: &PrefixAttestationCapability,
    expected_producer_id: &str,
    expected_turn_id: &str,
    raw_body: &str,
    attested: AttestedPrefixV1,
) -> SanitizationOutcome {
    let invalid = |reason| invalid_status(raw_body, expected_producer_id, expected_turn_id, reason);

    if !matches!(
        capability,
        PrefixAttestationCapability::SupportedV1 {
            issuer_id,
            boundary_scheme: PrefixBoundaryScheme::CodexCommitMarkerV1,
        } if *issuer_id == ATTESTED_PREFIX_ISSUER_V1
    ) {
        return invalid(InvalidAttestationReason::BackendCapabilityMismatch);
    }
    if attested.issuer_id != ATTESTED_PREFIX_ISSUER_V1 {
        return invalid(InvalidAttestationReason::UntrustedIssuer);
    }
    if attested.producer_id != expected_producer_id {
        return invalid(InvalidAttestationReason::ProducerMismatch);
    }
    if attested.turn_id != expected_turn_id {
        return invalid(InvalidAttestationReason::TurnMismatch);
    }

    let bytes = raw_body.as_bytes();
    let n = bytes.len();
    let Some(body_len) = usize::try_from(attested.body_len_bytes).ok() else {
        return invalid(InvalidAttestationReason::LengthMismatch);
    };
    if body_len != n {
        return invalid(InvalidAttestationReason::LengthMismatch);
    }
    if attested.body_sha256 != sha256(bytes) {
        return invalid(InvalidAttestationReason::DigestMismatch);
    }
    let Some(k) = usize::try_from(attested.process_prefix_bytes).ok() else {
        return invalid(InvalidAttestationReason::OffsetOverflow);
    };
    if k > n {
        return invalid(InvalidAttestationReason::OffsetOutOfBounds);
    }
    if k == n {
        return invalid(InvalidAttestationReason::EmptyDeliverable);
    }
    if !raw_body.is_char_boundary(k) {
        return invalid(InvalidAttestationReason::OffsetNotUtf8Boundary);
    }
    if (k as u128) * 100 > (n as u128) * 90 {
        return SanitizationOutcome {
            effective_body: raw_body.to_string(),
            status: PrefixAttestationStatus::AttestedV1(attested.clone()),
            decision: HarvestDecision::KeptSuspiciousAttestation,
            reason: Some("process_prefix_exceeds_90_percent".to_string()),
            cut_byte_offset: None,
            issuer_id: Some(attested.issuer_id),
        };
    }
    if k == 0 {
        return SanitizationOutcome {
            effective_body: raw_body.to_string(),
            status: PrefixAttestationStatus::AttestedV1(attested.clone()),
            decision: HarvestDecision::KeptZeroPrefix,
            reason: None,
            cut_byte_offset: Some(0),
            issuer_id: Some(attested.issuer_id),
        };
    }
    SanitizationOutcome {
        effective_body: raw_body[k..].to_string(),
        status: PrefixAttestationStatus::AttestedV1(attested.clone()),
        decision: HarvestDecision::CutAttested,
        reason: None,
        cut_byte_offset: Some(attested.process_prefix_bytes),
        issuer_id: Some(attested.issuer_id),
    }
}

fn provenance_sha256(
    context: &TurnContext,
    mode: HarvestSanitizationMode,
    capability: &PrefixAttestationCapability,
    producer_id: &str,
) -> Result<[u8; 32], HarvestAuditStoreError> {
    let value = serde_json::json!({
        "task_id": context.task_id.as_ref().map(|t| t.as_str()),
        "run_id": context.session_id.as_str(),
        "node_id": context.node.as_deref(),
        "attempt": context.attempt,
        "turn_id": context.turn_id.as_str(),
        "agent": context.agent.as_str(),
        "model": context.model.as_deref(),
        "effort": context.effort.as_deref(),
        "mode": mode,
        "capability": capability,
        "producer_id": producer_id,
        "prompt_id": context.prompt_id.as_deref(),
    });
    let bytes =
        serde_json::to_vec(&value).map_err(|e| HarvestAuditStoreError::Encoding(e.to_string()))?;
    Ok(sha256(&bytes))
}

fn context_task_id(context: &TurnContext) -> String {
    context
        .task_id
        .as_ref()
        .map(|task| task.as_str().to_string())
        .unwrap_or_else(|| context.session_id.as_str().to_string())
}

fn context_node_id(context: &TurnContext) -> String {
    context.node.clone().unwrap_or_else(|| "direct".to_string())
}

#[allow(clippy::too_many_arguments)]
pub async fn commit_harvested_completion(
    context: &TurnContext,
    mode: HarvestSanitizationMode,
    capability: &PrefixAttestationCapability,
    producer_id: &str,
    origin: CompletionBodyOrigin,
    raw_body: String,
    status: PrefixAttestationStatus,
    store: &dyn HarvestAuditStore,
) -> Result<CommittedHarvest, CompletionCommitError> {
    let task_id = context_task_id(context);
    let run_id = context.session_id.as_str().to_string();
    let node_id = context_node_id(context);
    let turn_id = context.turn_id.as_str().to_string();
    let audit_id =
        audit_id_for(&run_id, &node_id, context.attempt, &turn_id).map_err(|source| {
            CompletionCommitError::HarvestAuditPersistFailed {
                audit_id: "apc1_encoding_failed".to_string(),
                source,
            }
        })?;

    let outcome = sanitize_harvest(
        mode,
        origin,
        capability,
        producer_id,
        &turn_id,
        &raw_body,
        status,
    );
    let raw_len_bytes = raw_body.as_bytes().len() as u64;
    let effective_len_bytes = outcome.effective_body.as_bytes().len() as u64;
    let raw_body_sha256 = sha256(raw_body.as_bytes());
    let effective_body_sha256 = sha256(outcome.effective_body.as_bytes());
    let provenance_sha256 =
        provenance_sha256(context, mode, capability, producer_id).map_err(|source| {
            CompletionCommitError::HarvestAuditPersistFailed {
                audit_id: audit_id.clone(),
                source,
            }
        })?;

    let raw = HarvestRawRecordV1 {
        schema_version: HARVEST_SCHEMA_VERSION,
        audit_id: audit_id.clone(),
        task_id,
        run_id: run_id.clone(),
        node_id: node_id.clone(),
        attempt_id: context.attempt,
        turn_id: turn_id.clone(),
        backend_id: context.agent.clone(),
        producer_id: producer_id.to_string(),
        declared_capability: capability.clone(),
        raw_body,
        raw_len_bytes,
        raw_body_sha256,
        prefix_attestation: outcome.status.clone(),
        provenance_sha256,
    };
    let decision = HarvestSanitizationDecisionV1 {
        schema_version: HARVEST_SCHEMA_VERSION,
        audit_id: audit_id.clone(),
        mode,
        decision: outcome.decision,
        reason: outcome.reason,
        node_id,
        producer_id: producer_id.to_string(),
        issuer_id: outcome.issuer_id,
        raw_body_sha256,
        effective_body_sha256,
        raw_len_bytes,
        effective_len_bytes,
        cut_byte_offset: outcome.cut_byte_offset,
        provenance_sha256,
        suspicious_threshold_percent: SUSPICIOUS_THRESHOLD_PERCENT,
    };

    store
        .commit_bundle(raw, decision.clone())
        .await
        .map_err(|source| CompletionCommitError::HarvestAuditPersistFailed {
            audit_id: audit_id.clone(),
            source,
        })?;

    Ok(CommittedHarvest {
        audit_id,
        effective_body: outcome.effective_body,
        decision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attestation::{NoAttestationV1, RejectedAttestation};

    fn attested(body: &str, k: u64) -> PrefixAttestationStatus {
        PrefixAttestationStatus::AttestedV1(AttestedPrefixV1 {
            issuer_id: ATTESTED_PREFIX_ISSUER_V1.to_string(),
            producer_id: "producer".to_string(),
            turn_id: "turn_00000000000000000000000000000000".to_string(),
            body_len_bytes: body.len() as u64,
            body_sha256: sha256(body.as_bytes()),
            process_prefix_bytes: k,
        })
    }

    fn decide(body: &str, status: PrefixAttestationStatus) -> SanitizationOutcome {
        sanitize_harvest(
            HarvestSanitizationMode::AttestedPrefixV1,
            CompletionBodyOrigin::ModelText,
            &PrefixAttestationCapability::codex_commit_marker_v1(),
            "producer",
            "turn_00000000000000000000000000000000",
            body,
            status,
        )
    }

    #[test]
    fn off_returns_byte_identical_input() {
        let body = " bom:\u{feff}\r\nnul:\0 emoji:🦀";
        let out = sanitize_harvest(
            HarvestSanitizationMode::Off,
            CompletionBodyOrigin::ModelText,
            &PrefixAttestationCapability::default(),
            "producer",
            "turn_00000000000000000000000000000000",
            body,
            PrefixAttestationStatus::default(),
        );
        assert_eq!(out.effective_body, body);
        assert_eq!(out.decision, HarvestDecision::KeptOff);
    }

    #[test]
    fn successful_cut_returns_exact_suffix_and_zero_prefix_keeps() {
        let out = decide("proc\nDELIVER", attested("proc\nDELIVER", 5));
        assert_eq!(out.effective_body, "DELIVER");
        assert_eq!(out.decision, HarvestDecision::CutAttested);
        assert_eq!(out.cut_byte_offset, Some(5));

        let zero = decide("DELIVER", attested("DELIVER", 0));
        assert_eq!(zero.effective_body, "DELIVER");
        assert_eq!(zero.decision, HarvestDecision::KeptZeroPrefix);
        assert_eq!(zero.cut_byte_offset, Some(0));
    }

    #[test]
    fn suspicious_guard_allows_exactly_90_and_keeps_above_90() {
        let exact = decide("0123456789", attested("0123456789", 9));
        assert_eq!(exact.decision, HarvestDecision::CutAttested);
        assert_eq!(exact.effective_body, "9");

        let above = decide("01234567890", attested("01234567890", 10));
        assert_eq!(above.decision, HarvestDecision::KeptSuspiciousAttestation);
        assert_eq!(above.effective_body, "01234567890");
        assert_eq!(
            above.reason.as_deref(),
            Some("process_prefix_exceeds_90_percent")
        );
    }

    #[test]
    fn utf8_interior_offset_is_invalid_but_boundary_before_emoji_cuts() {
        let body = "a🦀z";
        let interior = decide(body, attested(body, 2));
        assert_eq!(interior.decision, HarvestDecision::KeptInvalidAttestation);
        assert_eq!(interior.reason.as_deref(), Some("offset_not_utf8_boundary"));
        assert_eq!(interior.effective_body, body);

        let boundary = decide(body, attested(body, 1));
        assert_eq!(boundary.effective_body, "🦀z");
    }

    #[test]
    fn invalid_identity_length_hash_and_capability_keep() {
        let mut wrong = match attested("abc", 1) {
            PrefixAttestationStatus::AttestedV1(a) => a,
            _ => unreachable!(),
        };
        wrong.issuer_id = "evil".to_string();
        assert_eq!(
            decide("abc", PrefixAttestationStatus::AttestedV1(wrong))
                .reason
                .as_deref(),
            Some("untrusted_issuer")
        );

        let unsupported = sanitize_harvest(
            HarvestSanitizationMode::AttestedPrefixV1,
            CompletionBodyOrigin::ModelText,
            &PrefixAttestationCapability::default(),
            "producer",
            "turn_00000000000000000000000000000000",
            "abc",
            attested("abc", 1),
        );
        assert_eq!(
            unsupported.reason.as_deref(),
            Some("backend_capability_mismatch")
        );

        let mut digest_bad = match attested("abc", 1) {
            PrefixAttestationStatus::AttestedV1(a) => a,
            _ => unreachable!(),
        };
        digest_bad.body_sha256 = [7; 32];
        assert_eq!(
            decide("abc", PrefixAttestationStatus::AttestedV1(digest_bad))
                .reason
                .as_deref(),
            Some("digest_mismatch")
        );
    }

    #[test]
    fn unavailable_rejected_and_synthetic_origins_keep() {
        let unavailable = PrefixAttestationStatus::UnavailableV1(NoAttestationV1 {
            producer_id: "producer".to_string(),
            turn_id: "turn_00000000000000000000000000000000".to_string(),
            reason: NoAttestationReason::TurnMissingDeliverableBoundary,
        });
        let out = decide("full", unavailable);
        assert_eq!(out.effective_body, "full");
        assert_eq!(out.decision, HarvestDecision::KeptNoAttestation);
        assert_eq!(
            out.reason.as_deref(),
            Some("turn_missing_deliverable_boundary")
        );

        let rejected = PrefixAttestationStatus::Rejected(RejectedAttestation {
            producer_id: "producer".to_string(),
            turn_id: "turn_00000000000000000000000000000000".to_string(),
            reason: InvalidAttestationReason::MalformedMetadata,
        });
        let out = decide("full", rejected);
        assert_eq!(out.decision, HarvestDecision::KeptInvalidAttestation);
        assert_eq!(out.reason.as_deref(), Some("malformed_metadata"));

        let synthetic = sanitize_harvest(
            HarvestSanitizationMode::AttestedPrefixV1,
            CompletionBodyOrigin::BridgeSyntheticCancellation,
            &PrefixAttestationCapability::codex_commit_marker_v1(),
            "producer",
            "turn_00000000000000000000000000000000",
            "[node a canceled]",
            attested("[node a canceled]", 1),
        );
        assert_eq!(synthetic.effective_body, "[node a canceled]");
        assert_eq!(synthetic.decision, HarvestDecision::KeptNoAttestation);
        assert_eq!(
            synthetic.reason.as_deref(),
            Some("bridge_synthetic_cancellation")
        );
    }

    #[test]
    fn audit_id_matches_known_vector() {
        assert_eq!(
            audit_id_for("run", "node", 7, "turn_00000000000000000000000000000000").unwrap(),
            "apc1_e860b2f7825b41eddf22faaf9ba00edfdf80e542e286757fd66cd4a84db7c89f"
        );
    }
}
