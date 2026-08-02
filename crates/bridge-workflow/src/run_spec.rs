//! Persisted R2f1a workflow execution identity.
//!
//! A V2 run is constructed once before task/history/session/provider effects. Resume decodes and
//! revalidates these exact bytes; it never re-resolves controls or replaces provider identity from
//! current configuration.

use crate::graph::{WorkflowGraph, WorkflowNode};
use bridge_core::execution_policy::{
    validate_node_execution_identity_v1, FrozenNodeExecutionIdentityV1, FrozenWorkflowControlsV1,
    LedgerAdmissionV1, PolicyNodeRefV1, Sha256HexV1, EXECUTION_POLICY_SCHEMA_V1,
};
use bridge_core::ids::AttemptId;
use bridge_core::SessionCwd;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const WORKFLOW_SNAPSHOT_V2: u16 = 2;
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
            Self::SnapshotEncoding => "invalid workflow snapshot V2 encoding",
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
