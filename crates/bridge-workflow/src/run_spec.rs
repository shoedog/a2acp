//! Persisted R2f1a workflow execution identity.
//!
//! A V2 run is constructed once before task/history/session/provider effects. Resume decodes and
//! revalidates these exact bytes; it never re-resolves controls or replaces provider identity from
//! current configuration.

use crate::graph::{WorkflowGraph, WorkflowNode};
use bridge_core::execution_policy::{
    validate_node_execution_identity_v1, DeadlineActivationV2, FrozenNodeExecutionIdentityV1,
    FrozenR2f1bContractV1, FrozenWorkflowControlsV1, LedgerAdmissionV1, PolicyNodeRefV1,
    Sha256HexV1, EXECUTION_POLICY_SCHEMA_V1,
};
use bridge_core::ids::{AttemptId, AttemptIdentity};
use bridge_core::SessionCwd;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const WORKFLOW_SNAPSHOT_V2: u16 = 2;
pub const WORKFLOW_SNAPSHOT_V3: u16 = 3;
pub const MAX_V2_RETRY_ATTEMPTS: u32 = 1_024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunSpecError {
    UnsupportedSchema,
    InvalidGraph,
    InvalidControls,
    InvalidNodeIdentity,
    InvalidRetry,
    RetryBudgetExceeded,
    FingerprintMismatch,
    SnapshotEncoding,
    InvalidAttemptLineage,
    DeliverySpecChanged,
    InvalidR2f1bContract,
}

impl std::fmt::Display for RunSpecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedSchema => "unsupported R2f1a schema",
            Self::InvalidGraph => "invalid frozen workflow graph",
            Self::InvalidControls => "invalid frozen workflow controls",
            Self::InvalidNodeIdentity => "invalid frozen node execution identity",
            Self::InvalidRetry => "invalid frozen retry policy",
            Self::RetryBudgetExceeded => "critical-path retry backoff exceeds the work cutoff",
            Self::FingerprintMismatch => "frozen workflow fingerprint mismatch",
            Self::SnapshotEncoding => "invalid workflow snapshot encoding",
            Self::InvalidAttemptLineage => "invalid workflow snapshot attempt lineage",
            Self::DeliverySpecChanged => "workflow snapshot delivery spec changed",
            Self::InvalidR2f1bContract => "invalid R2f1b contract",
        })
    }
}

impl std::error::Error for RunSpecError {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRunSpecV1 {
    pub schema_version: u16,
    pub attempt_id: AttemptId,
    pub graph: WorkflowGraph,
    pub controls: FrozenWorkflowControlsV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_session_cwd: Option<SessionCwd>,
    pub node_execution_identities: Vec<FrozenNodeExecutionIdentityV1>,
    pub ledger_admission: LedgerAdmissionV1,
    pub controls_fingerprint: String,
    pub workload_fingerprint: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowSnapshotEnvelopeV2 {
    v: u16,
    run_spec: WorkflowRunSpecV1,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowSnapshotEnvelopeV3 {
    v: u16,
    attempt: AttemptIdentity,
    delivery_spec: WorkflowRunSpecV1,
    predecessor_snapshot_digest: Option<Sha256HexV1>,
    r2f1b: FrozenR2f1bContractV1,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowSnapshotV3 {
    pub attempt: AttemptIdentity,
    pub delivery_spec: WorkflowRunSpecV1,
    pub predecessor_snapshot_digest: Option<Sha256HexV1>,
    pub r2f1b: FrozenR2f1bContractV1,
}

impl WorkflowSnapshotV3 {
    pub fn validate(&self) -> Result<(), RunSpecError> {
        self.delivery_spec.validate()?;
        self.r2f1b
            .validate()
            .map_err(|_| RunSpecError::InvalidR2f1bContract)?;
        match (self.attempt.ordinal, &self.attempt.parent_attempt_id) {
            (0, None) => {
                if self.predecessor_snapshot_digest.is_some()
                    || self.attempt.attempt_id != self.delivery_spec.attempt_id
                {
                    return Err(RunSpecError::InvalidAttemptLineage);
                }
            }
            (0, Some(_)) | (_, None) => return Err(RunSpecError::InvalidAttemptLineage),
            (_, Some(parent)) => {
                if self.predecessor_snapshot_digest.is_none()
                    || parent == &self.attempt.attempt_id
                    || self.attempt.attempt_id == self.delivery_spec.attempt_id
                {
                    return Err(RunSpecError::InvalidAttemptLineage);
                }
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, RunSpecError> {
        self.validate()?;
        serde_json::to_vec(&WorkflowSnapshotEnvelopeV3 {
            v: WORKFLOW_SNAPSHOT_V3,
            attempt: self.attempt.clone(),
            delivery_spec: self.delivery_spec.clone(),
            predecessor_snapshot_digest: self.predecessor_snapshot_digest.clone(),
            r2f1b: self.r2f1b.clone(),
        })
        .map_err(|_| RunSpecError::SnapshotEncoding)
    }

    pub fn digest(&self) -> Result<Sha256HexV1, RunSpecError> {
        Ok(Sha256HexV1::digest(&self.encode()?))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RunSpecError> {
        let envelope: WorkflowSnapshotEnvelopeV3 =
            serde_json::from_slice(bytes).map_err(|_| RunSpecError::SnapshotEncoding)?;
        if envelope.v != WORKFLOW_SNAPSHOT_V3 {
            return Err(RunSpecError::UnsupportedSchema);
        }
        let snapshot = Self {
            attempt: envelope.attempt,
            delivery_spec: envelope.delivery_spec,
            predecessor_snapshot_digest: envelope.predecessor_snapshot_digest,
            r2f1b: envelope.r2f1b,
        };
        snapshot.validate()?;
        if snapshot.encode()? != bytes {
            return Err(RunSpecError::SnapshotEncoding);
        }
        Ok(snapshot)
    }

    pub fn validate_successor(predecessor: &Self, successor: &Self) -> Result<(), RunSpecError> {
        predecessor.validate()?;
        successor.validate()?;
        if successor.attempt.attempt_id == predecessor.attempt.attempt_id
            || successor.attempt.execution_id != predecessor.attempt.execution_id
            || successor.attempt.ordinal
                != predecessor
                    .attempt
                    .ordinal
                    .checked_add(1)
                    .ok_or(RunSpecError::InvalidAttemptLineage)?
            || successor.attempt.parent_attempt_id.as_ref() != Some(&predecessor.attempt.attempt_id)
            || successor.predecessor_snapshot_digest.as_ref() != Some(&predecessor.digest()?)
        {
            return Err(RunSpecError::InvalidAttemptLineage);
        }
        let predecessor_delivery = serde_json::to_vec(&predecessor.delivery_spec)
            .map_err(|_| RunSpecError::SnapshotEncoding)?;
        let successor_delivery = serde_json::to_vec(&successor.delivery_spec)
            .map_err(|_| RunSpecError::SnapshotEncoding)?;
        if predecessor_delivery != successor_delivery || predecessor.r2f1b != successor.r2f1b {
            return Err(RunSpecError::DeliverySpecChanged);
        }
        Ok(())
    }

    /// The effective workload/calibration identity of this attempt (R2f1b gate
    /// §2.6).
    ///
    /// The frozen delivery-spec `workload_fingerprint` commits graph, controls,
    /// retries, and frozen node/provider identities, but the R2f1b contract is
    /// wrapped OUTSIDE it. For an automatically timed attempt the identity must
    /// additionally commit the explicit activation and the validated contract
    /// fingerprint, so automatic observations can never be pooled with manual
    /// ones (or with a different custody/resource contract).
    ///
    /// `ManualOnlyR2f1a` returns the historical fingerprint verbatim, and that
    /// is deliberate on two grounds. It is a compatibility contract: persisted
    /// history/calibration rows already group by that exact string, so binding
    /// it would silently re-partition every historical grouping. It is also the
    /// only grouping that survives at all — `WorktreeCustodyIdV1` is randomly
    /// minted per attempt and feeds `contract_fingerprint`, so a bound manual
    /// identity would be unique per attempt and pool nothing. Manual attempts
    /// remain protected against custody substitution by
    /// `FrozenR2f1bContractV1::validate` and by `validate_successor`'s
    /// byte-exact contract comparison, not by the workload identity.
    ///
    /// Both inputs are persisted, forgeable fields, so this refuses rather than
    /// computing over unverified bytes: `validate` re-derives the delivery
    /// fingerprints and revalidates the contract fingerprint first.
    pub fn workload_identity(&self) -> Result<String, RunSpecError> {
        self.validate()?;
        Ok(match self.r2f1b.activation {
            DeadlineActivationV2::ManualOnlyR2f1a => {
                self.delivery_spec.workload_fingerprint.clone()
            }
            DeadlineActivationV2::AutomaticR2f1b => bound_workload_fingerprint_v2(
                &self.delivery_spec.workload_fingerprint,
                self.r2f1b.activation,
                &self.r2f1b.contract_fingerprint,
            ),
        })
    }
}

/// Domain prefix for the R2f1b-bound workload identity. Separated from BOTH the
/// manual computation (`a2a-bridge/workflow-run-spec/v1`, emitted under the
/// `shape-` prefix that `bridge_core::workflow_history::fingerprint_workload_shape`
/// also emits) and the contract-hash algorithm (`FrozenR2f1bContractV1`'s
/// unprefixed canonical-JSON digest).
const BOUND_WORKLOAD_DOMAIN_V2: &[u8] = b"a2a-bridge/workflow-run-spec/bound-workload/v2";

/// Version 2 of the workload identity: the frozen V1 workload fingerprint bound
/// to an explicit deadline activation and a validated R2f1b contract
/// fingerprint, under its own domain and its own `bound-` output prefix.
///
/// `activation` is committed EXPLICITLY even though it is also an input to
/// `contract_fingerprint`: gate §2.6 keeps the manual/automatic semantic
/// boundary auditable in this buffer, independent of the contract-hash
/// algorithm. Callers wanting the identity of a durable attempt should use
/// [`WorkflowSnapshotV3::workload_identity`], which selects by activation and
/// validates both inputs first; this free function is the raw computation.
#[must_use]
pub fn bound_workload_fingerprint_v2(
    workload_fingerprint: &str,
    activation: DeadlineActivationV2,
    contract_fingerprint: &Sha256HexV1,
) -> String {
    let mut canonical = Vec::new();
    push_bytes(&mut canonical, BOUND_WORKLOAD_DOMAIN_V2);
    push_bytes(&mut canonical, workload_fingerprint.as_bytes());
    push_bytes(
        &mut canonical,
        match activation {
            DeadlineActivationV2::ManualOnlyR2f1a => b"manual_only_r2f1a".as_slice(),
            DeadlineActivationV2::AutomaticR2f1b => b"automatic_r2f1b".as_slice(),
        },
    );
    push_bytes(&mut canonical, contract_fingerprint.as_str().as_bytes());
    digest_with_prefix("bound-", &canonical)
}

fn push_bytes(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}

fn digest_with_prefix(prefix: &str, bytes: &[u8]) -> String {
    format!("{prefix}{}", Sha256HexV1::digest(bytes).as_str())
}

fn controls_fingerprint(controls: &FrozenWorkflowControlsV1) -> Result<String, RunSpecError> {
    let bytes = serde_json::to_vec(controls).map_err(|_| RunSpecError::SnapshotEncoding)?;
    Ok(digest_with_prefix("controls-", &bytes))
}

fn encode_retry(target: &mut Vec<u8>, node: &WorkflowNode) {
    match &node.retry {
        Some(retry) => {
            target.push(1);
            target.extend_from_slice(&retry.max_attempts.to_be_bytes());
            target.extend_from_slice(&retry.backoff_ms.to_be_bytes());
            match retry.backoff_cap_ms {
                Some(cap) => {
                    target.push(1);
                    target.extend_from_slice(&cap.to_be_bytes());
                }
                None => target.push(0),
            }
        }
        None => target.push(0),
    }
}

fn workload_fingerprint(
    graph: &WorkflowGraph,
    controls_fingerprint: &str,
    identities: &[FrozenNodeExecutionIdentityV1],
) -> String {
    let mut canonical = Vec::new();
    push_bytes(&mut canonical, b"a2a-bridge/workflow-run-spec/v1");
    push_bytes(&mut canonical, graph.id.as_str().as_bytes());
    push_bytes(&mut canonical, controls_fingerprint.as_bytes());
    let mut nodes: Vec<_> = graph.nodes.iter().collect();
    nodes.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    canonical.extend_from_slice(&(nodes.len() as u64).to_be_bytes());
    for node in nodes {
        push_bytes(&mut canonical, node.id.as_str().as_bytes());
        push_bytes(&mut canonical, node.agent.as_str().as_bytes());
        let mut inputs: Vec<_> = node.inputs.iter().map(|input| input.as_str()).collect();
        inputs.sort_unstable();
        canonical.extend_from_slice(&(inputs.len() as u64).to_be_bytes());
        for input in inputs {
            push_bytes(&mut canonical, input.as_bytes());
        }
        encode_retry(&mut canonical, node);
        push_bytes(
            &mut canonical,
            match node.harvest_sanitization.unwrap_or_default() {
                bridge_core::attestation::HarvestSanitizationMode::Off => b"off",
                bridge_core::attestation::HarvestSanitizationMode::AttestedPrefixV1 => {
                    b"attested_prefix_v1"
                }
            },
        );
    }
    match &graph.panel {
        Some(panel) => {
            canonical.push(1);
            canonical.extend_from_slice(&(panel.weights.len() as u64).to_be_bytes());
            for (name, weight) in &panel.weights {
                push_bytes(&mut canonical, name.as_bytes());
                canonical.extend_from_slice(&weight.to_bits().to_be_bytes());
            }
        }
        None => canonical.push(0),
    }
    canonical.extend_from_slice(&(identities.len() as u64).to_be_bytes());
    for identity in identities {
        push_bytes(
            &mut canonical,
            identity.identity_fingerprint.as_str().as_bytes(),
        );
    }
    digest_with_prefix("shape-", &canonical)
}

fn retry_delay_ms(node: &WorkflowNode) -> Result<u64, RunSpecError> {
    let Some(retry) = &node.retry else {
        return Ok(0);
    };
    if !(1..=MAX_V2_RETRY_ATTEMPTS).contains(&retry.max_attempts) {
        return Err(RunSpecError::InvalidRetry);
    }
    let mut total = 0_u64;
    for attempt in 1..retry.max_attempts {
        let delay = u64::try_from(retry.backoff_for(attempt).as_millis())
            .map_err(|_| RunSpecError::InvalidRetry)?;
        total = total.checked_add(delay).ok_or(RunSpecError::InvalidRetry)?;
    }
    Ok(total)
}

fn validate_critical_path_retry_budget(
    graph: &WorkflowGraph,
    work_cutoff_ms: u64,
) -> Result<(), RunSpecError> {
    let mut remaining: Vec<_> = graph.nodes.iter().collect();
    let mut path_cost = HashMap::<&str, u64>::new();
    while !remaining.is_empty() {
        let before = remaining.len();
        let mut index = 0;
        while index < remaining.len() {
            let node = remaining[index];
            if node
                .inputs
                .iter()
                .all(|input| path_cost.contains_key(input.as_str()))
            {
                let parent = node
                    .inputs
                    .iter()
                    .filter_map(|input| path_cost.get(input.as_str()).copied())
                    .max()
                    .unwrap_or(0);
                let total = parent
                    .checked_add(retry_delay_ms(node)?)
                    .ok_or(RunSpecError::InvalidRetry)?;
                if total >= work_cutoff_ms {
                    return Err(RunSpecError::RetryBudgetExceeded);
                }
                path_cost.insert(node.id.as_str(), total);
                remaining.swap_remove(index);
            } else {
                index += 1;
            }
        }
        if remaining.len() == before {
            return Err(RunSpecError::InvalidGraph);
        }
    }
    Ok(())
}

impl WorkflowRunSpecV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        attempt_id: AttemptId,
        graph: WorkflowGraph,
        controls: FrozenWorkflowControlsV1,
        requested_session_cwd: Option<SessionCwd>,
        mut node_execution_identities: Vec<FrozenNodeExecutionIdentityV1>,
        ledger_admission: LedgerAdmissionV1,
    ) -> Result<Self, RunSpecError> {
        node_execution_identities.sort_by_key(|identity| identity.node.sorted_ordinal);
        let controls_fingerprint = controls_fingerprint(&controls)?;
        let workload_fingerprint =
            workload_fingerprint(&graph, &controls_fingerprint, &node_execution_identities);
        let spec = Self {
            schema_version: EXECUTION_POLICY_SCHEMA_V1,
            attempt_id,
            graph,
            controls,
            requested_session_cwd,
            node_execution_identities,
            ledger_admission,
            controls_fingerprint,
            workload_fingerprint,
        };
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<(), RunSpecError> {
        if self.schema_version != EXECUTION_POLICY_SCHEMA_V1 {
            return Err(RunSpecError::UnsupportedSchema);
        }
        self.graph
            .validate()
            .map_err(|_| RunSpecError::InvalidGraph)?;
        if self.controls.schema_version != EXECUTION_POLICY_SCHEMA_V1
            || self.controls.profile.schema_version != EXECUTION_POLICY_SCHEMA_V1
            || self.controls.effective_terminal_bound_ms().is_err()
        {
            return Err(RunSpecError::InvalidControls);
        }
        validate_critical_path_retry_budget(&self.graph, self.controls.effective_work_cutoff_ms())?;

        let mut nodes: Vec<_> = self.graph.nodes.iter().collect();
        nodes.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        if nodes.len() != self.node_execution_identities.len() || nodes.len() > u32::MAX as usize {
            return Err(RunSpecError::InvalidNodeIdentity);
        }
        for (ordinal, (node, identity)) in nodes
            .into_iter()
            .zip(&self.node_execution_identities)
            .enumerate()
        {
            let ordinal = u32::try_from(ordinal).map_err(|_| RunSpecError::InvalidNodeIdentity)?;
            if identity.node != PolicyNodeRefV1::from_node_id(ordinal, node.id.as_str())
                || identity.selection.agent != node.agent
                || validate_node_execution_identity_v1(identity).is_err()
            {
                return Err(RunSpecError::InvalidNodeIdentity);
            }
        }

        let controls_fingerprint = controls_fingerprint(&self.controls)?;
        let workload_fingerprint = workload_fingerprint(
            &self.graph,
            &controls_fingerprint,
            &self.node_execution_identities,
        );
        if self.controls_fingerprint != controls_fingerprint
            || self.workload_fingerprint != workload_fingerprint
        {
            return Err(RunSpecError::FingerprintMismatch);
        }
        Ok(())
    }

    pub fn encode_snapshot_v3(
        &self,
        attempt: AttemptIdentity,
        r2f1b: FrozenR2f1bContractV1,
    ) -> Result<Vec<u8>, RunSpecError> {
        if attempt.ordinal != 0
            || attempt.parent_attempt_id.is_some()
            || attempt.attempt_id != self.attempt_id
        {
            return Err(RunSpecError::InvalidAttemptLineage);
        }
        WorkflowSnapshotV3 {
            attempt,
            delivery_spec: self.clone(),
            predecessor_snapshot_digest: None,
            r2f1b,
        }
        .encode()
    }

    pub fn decode_snapshot_v3(bytes: &[u8]) -> Result<WorkflowSnapshotV3, RunSpecError> {
        WorkflowSnapshotV3::decode(bytes)
    }

    pub fn encode_snapshot_v2(&self) -> Result<Vec<u8>, RunSpecError> {
        self.validate()?;
        serde_json::to_vec(&WorkflowSnapshotEnvelopeV2 {
            v: WORKFLOW_SNAPSHOT_V2,
            run_spec: self.clone(),
        })
        .map_err(|_| RunSpecError::SnapshotEncoding)
    }

    pub fn decode_snapshot_v2(bytes: &[u8]) -> Result<Self, RunSpecError> {
        let envelope: WorkflowSnapshotEnvelopeV2 =
            serde_json::from_slice(bytes).map_err(|_| RunSpecError::SnapshotEncoding)?;
        if envelope.v != WORKFLOW_SNAPSHOT_V2 {
            return Err(RunSpecError::UnsupportedSchema);
        }
        envelope.run_spec.validate()?;
        Ok(envelope.run_spec)
    }
}
