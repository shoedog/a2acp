//! Pre-effect construction of persisted R2f1a workflow authority.

use crate::graph::WorkflowGraph;
use crate::run_spec::WorkflowRunSpecV1;
use async_trait::async_trait;
use bridge_core::domain::AgentEntry;
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{
    freeze_node_execution_identity_v1, freeze_provider_attempt_v1, resolve_execution_policy_v1,
    select_custody_plan_v1, DeadlineActivationV2, ExecutionPolicyInvocationV1,
    FrozenCheckoutEffectV1, FrozenProviderLogicalSessionV1, FrozenR2f1bContractV1,
    LedgerAdmissionV1, PolicyActivationV1, PolicyNodeRefV1, ProviderEffectKeyV1,
    ProviderFreezeInputV1, WorkflowControlDefaultsV1,
};
use bridge_core::ids::{AttemptId, AttemptIdentity};
use bridge_core::ports::AgentRegistry;
use bridge_core::SessionCwd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct CheckoutPlanInputV1 {
    pub attempt_id: AttemptId,
    pub node: PolicyNodeRefV1,
    pub logical_session: FrozenProviderLogicalSessionV1,
    pub source_cwd: SessionCwd,
}

#[async_trait]
pub trait WorkflowCheckoutPlannerV1: Send + Sync {
    async fn freeze_checkout(
        &self,
        entry: &AgentEntry,
        input: &CheckoutPlanInputV1,
    ) -> Result<FrozenCheckoutEffectV1, BridgeError>;
}

#[derive(Default)]
pub struct DirectWorkflowCheckoutPlannerV1;

#[async_trait]
impl WorkflowCheckoutPlannerV1 for DirectWorkflowCheckoutPlannerV1 {
    async fn freeze_checkout(
        &self,
        _entry: &AgentEntry,
        input: &CheckoutPlanInputV1,
    ) -> Result<FrozenCheckoutEffectV1, BridgeError> {
        Ok(bridge_core::execution_policy::freeze_direct_checkout_v1(
            input.source_cwd.clone(),
        ))
    }
}

#[derive(Clone)]
pub struct WorkflowAdmissionV1 {
    registry: Arc<dyn AgentRegistry>,
    checkout_planner: Arc<dyn WorkflowCheckoutPlannerV1>,
    launch_cwd: SessionCwd,
    provider_effect_key: Option<Arc<ProviderEffectKeyV1>>,
    provider_effect_key_path: Option<PathBuf>,
}

#[derive(Clone)]
pub struct AdmittedWorkflowRunV1 {
    pub run_spec: Arc<WorkflowRunSpecV1>,
    pub provider_effect_key: Option<Arc<ProviderEffectKeyV1>>,
    /// The admitted R2f1b contract, or `None` for every V2 run.
    ///
    /// Production admission has no way to populate this: every caller of [`WorkflowAdmissionV1::
    /// freeze`] passes `r2f1b: None` (see the §2c unreachability argument in the slice handoff),
    /// and the one activation that could arm timers is refused outright by
    /// [`admit_r2f1b_contract_v1`]. Slice 4 owns making it reachable.
    pub r2f1b: Option<Arc<R2f1bAdmissionV1>>,
}

/// One R2f1b contract offered to admission, with the attempt that will execute it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct R2f1bAdmissionV1 {
    pub attempt: AttemptIdentity,
    pub contract: FrozenR2f1bContractV1,
}

/// The single production admission rule for an R2f1b contract (slice-2 brief §3, Sol 13).
///
/// **Deliberately NOT in [`FrozenR2f1bContractV1::validate`].** Putting the activation refusal in
/// `validate` would make `with_computed_fingerprint` fail for `AutomaticR2f1b` and break the
/// already-landed A3/A5 offline construction, encoding, decoding, and workload-identity tests —
/// which must stay legal, because slice 4 needs to build and persist automatic contracts before
/// it is allowed to run them. The refusal belongs at the boundary where a contract becomes
/// *effective*, and this is that boundary.
///
/// It is also half of the reachability argument the whole slice rests on: with automatic
/// activation refused here and no production caller offering a `ManualOnlyR2f1a` contract, no
/// production path can route a custody plan to the backend at all.
pub fn admit_r2f1b_contract_v1(
    attempt_id: &AttemptId,
    admission: &R2f1bAdmissionV1,
) -> Result<(), BridgeError> {
    admission.contract.validate().map_err(|error| {
        invalid(&format!(
            "R2f1b contract is not admissible: {error}, contract is invalid"
        ))
    })?;
    if admission.contract.activation == DeadlineActivationV2::AutomaticR2f1b {
        return Err(invalid(
            "automatic R2f1b deadline activation is refused at admission: no slice-2 \
             production path may arm it",
        ));
    }
    // Fresh admission only. Resume mints a successor identity and exchanges the custody claim —
    // §5.8, owned by 2d (mechanism) and slice 5 (production). A successor arriving here would
    // route a claim-less V3 write over a predecessor's live checkout.
    if admission.attempt.ordinal != 0
        || admission.attempt.parent_attempt_id.is_some()
        || &admission.attempt.attempt_id != attempt_id
    {
        return Err(invalid(
            "R2f1b admission accepts only a fresh attempt identity matching the run attempt",
        ));
    }
    Ok(())
}

pub struct WorkflowAdmissionRequestV1 {
    pub attempt_id: AttemptId,
    pub graph: Arc<WorkflowGraph>,
    pub requested_session_cwd: Option<SessionCwd>,
    pub policy_invocation: ExecutionPolicyInvocationV1,
    pub ledger_admission: LedgerAdmissionV1,
    /// The R2f1b contract for this run. **Every production construction site passes `None`** —
    /// the field is explicit rather than defaulted precisely so that stays greppable.
    pub r2f1b: Option<R2f1bAdmissionV1>,
}

impl WorkflowAdmissionV1 {
    pub fn new(
        registry: Arc<dyn AgentRegistry>,
        checkout_planner: Arc<dyn WorkflowCheckoutPlannerV1>,
        launch_cwd: SessionCwd,
        provider_effect_key: Option<Arc<ProviderEffectKeyV1>>,
    ) -> Self {
        Self {
            registry,
            checkout_planner,
            launch_cwd,
            provider_effect_key,
            provider_effect_key_path: None,
        }
    }

    #[must_use]
    pub fn with_provider_effect_key_path(mut self, path: PathBuf) -> Self {
        self.provider_effect_key_path = Some(path);
        self
    }

    pub async fn freeze(
        &self,
        request: WorkflowAdmissionRequestV1,
    ) -> Result<AdmittedWorkflowRunV1, BridgeError> {
        request
            .graph
            .validate()
            .map_err(|_| invalid("workflow graph is invalid"))?;
        // Before any freezing, so a refused activation admits nothing at all.
        if let Some(r2f1b) = &request.r2f1b {
            admit_r2f1b_contract_v1(&request.attempt_id, r2f1b)?;
        }

        let mut nodes: Vec<_> = request.graph.nodes.iter().collect();
        nodes.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        let entries: Vec<_> = nodes
            .iter()
            .map(|node| {
                self.registry
                    .entry_snapshot(&node.agent)
                    .ok_or_else(|| invalid("workflow agent has no immutable registry snapshot"))
            })
            .collect::<Result<_, _>>()?;
        let any_max_effort = entries.iter().any(|entry| {
            bridge_core::domain::effective_config(entry, None).effort
                == Some(bridge_core::domain::Effort::Max)
        });
        let controls = resolve_execution_policy_v1(
            request
                .graph
                .controls
                .as_ref()
                .unwrap_or(&WorkflowControlDefaultsV1::default()),
            &request.policy_invocation,
            any_max_effort,
            PolicyActivationV1::Production,
        )
        .map_err(|error| invalid(&format!("workflow controls are invalid: {error}")))?;

        let mut identities = Vec::with_capacity(nodes.len());
        for (ordinal, (node, entry)) in nodes.into_iter().zip(entries).enumerate() {
            let sorted_ordinal = u32::try_from(ordinal)
                .map_err(|_| invalid("workflow has too many nodes for frozen identity"))?;
            let node_ref = PolicyNodeRefV1::from_node_id(sorted_ordinal, node.id.as_str());
            let source_cwd = resolve_source_cwd(
                request.requested_session_cwd.as_ref(),
                &entry,
                &self.launch_cwd,
            )?;
            self.reject_key_overlap(&source_cwd)?;
            let candidate_count = if entry.preflight {
                entry
                    .fallback_models
                    .len()
                    .checked_add(1)
                    .ok_or_else(|| invalid("provider candidate count overflow"))?
            } else {
                1
            };
            let mut logical_sessions = Vec::with_capacity(if entry.preflight {
                candidate_count
                    .checked_mul(2)
                    .ok_or_else(|| invalid("provider attempt count overflow"))?
            } else {
                1
            });
            for candidate in 0..candidate_count {
                let candidate_ordinal = u16::try_from(candidate)
                    .map_err(|_| invalid("provider candidate count exceeds frozen schema"))?;
                if entry.preflight {
                    logical_sessions
                        .push(FrozenProviderLogicalSessionV1::Preflight { candidate_ordinal });
                }
                logical_sessions
                    .push(FrozenProviderLogicalSessionV1::Execute { candidate_ordinal });
            }

            let mut bundles = Vec::with_capacity(logical_sessions.len());
            for logical_session in logical_sessions {
                let checkout = self
                    .checkout_planner
                    .freeze_checkout(
                        &entry,
                        &CheckoutPlanInputV1 {
                            attempt_id: request.attempt_id.clone(),
                            node: node_ref.clone(),
                            logical_session,
                            source_cwd: source_cwd.clone(),
                        },
                    )
                    .await?;
                bundles.push(
                    freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
                        entry: &entry,
                        overrides: None,
                        node: node_ref.clone(),
                        logical_session,
                        checkout,
                        provider_effect_key: self.provider_effect_key.as_deref(),
                    })
                    .map_err(|error| {
                        invalid(&format!("provider attempt cannot be frozen: {error}"))
                    })?,
                );
            }
            identities.push(
                freeze_node_execution_identity_v1(node_ref, bundles).map_err(|error| {
                    invalid(&format!(
                        "node execution identity cannot be frozen: {error}"
                    ))
                })?,
            );
        }

        let run_spec = Arc::new(
            WorkflowRunSpecV1::build(
                request.attempt_id,
                (*request.graph).clone(),
                controls,
                request.requested_session_cwd,
                identities,
                request.ledger_admission,
            )
            .map_err(|error| invalid(&format!("workflow run specification is invalid: {error}")))?,
        );
        // Coverage, checked once at admission rather than per node at bind time: §2.2 requires
        // `custody_plans` to enumerate EVERY worktree checkout in the candidate matrix. Deferring
        // this to the executor would let a run start and then refuse mid-graph, after siblings
        // had already materialized.
        if let Some(r2f1b) = &request.r2f1b {
            for identity in &run_spec.node_execution_identities {
                for attempt in &identity.provider_attempts {
                    select_custody_plan_v1(&r2f1b.contract, &attempt.checkout).map_err(
                        |error| {
                            invalid(&format!(
                                "R2f1b custody plan coverage is incomplete: {error}"
                            ))
                        },
                    )?;
                }
            }
        }
        Ok(AdmittedWorkflowRunV1 {
            run_spec,
            provider_effect_key: self.provider_effect_key.clone(),
            r2f1b: request.r2f1b.map(Arc::new),
        })
    }

    pub fn restore(
        &self,
        run_spec: WorkflowRunSpecV1,
    ) -> Result<AdmittedWorkflowRunV1, BridgeError> {
        run_spec.validate().map_err(|error| {
            invalid(&format!(
                "persisted workflow run specification is invalid: {error}"
            ))
        })?;
        for identity in &run_spec.node_execution_identities {
            for attempt in &identity.provider_attempts {
                self.reject_key_overlap(attempt.checkout.source_cwd())?;
                self.reject_key_overlap(attempt.checkout.effective_cwd())?;
            }
        }
        for identity in &run_spec.node_execution_identities {
            for attempt in &identity.provider_attempts {
                if let Some(expected) = &attempt.effect.secret_commitment_key_id {
                    let actual = self
                        .provider_effect_key
                        .as_ref()
                        .map(|key| key.key_id())
                        .ok_or_else(|| {
                            invalid("persisted provider effect requires its commitment key")
                        })?;
                    if &actual != expected {
                        return Err(BridgeError::ConfigMismatch {
                            field: "provider_effect_key_id",
                        });
                    }
                }
            }
        }
        Ok(AdmittedWorkflowRunV1 {
            run_spec: Arc::new(run_spec),
            provider_effect_key: self.provider_effect_key.clone(),
            // A restored V2 run spec carries no R2f1b contract by construction: the contract is
            // wrapped OUTSIDE `WorkflowRunSpecV1` (in `WorkflowSnapshotV3`), and this entry takes
            // only the V2 spec. Restoring a V3 snapshot is slice 5's.
            r2f1b: None,
        })
    }

    fn reject_key_overlap(&self, cwd: &SessionCwd) -> Result<(), BridgeError> {
        if self
            .provider_effect_key_path
            .as_ref()
            .is_some_and(|key| key == Path::new(cwd.as_str()) || key.starts_with(cwd.as_str()))
        {
            return Err(invalid(
                "provider-effect key path overlaps the workflow session root",
            ));
        }
        Ok(())
    }
}

fn invalid(reason: &str) -> BridgeError {
    BridgeError::ConfigInvalid {
        reason: reason.to_owned(),
    }
}

fn resolve_source_cwd(
    requested: Option<&SessionCwd>,
    entry: &AgentEntry,
    launch_cwd: &SessionCwd,
) -> Result<SessionCwd, BridgeError> {
    if let Some(requested) = requested {
        return Ok(requested.clone());
    }
    let Some(configured) = entry.session_cwd.as_deref().or(entry.cwd.as_deref()) else {
        return Ok(launch_cwd.clone());
    };
    let configured_path = Path::new(configured);
    if configured_path.is_absolute() {
        SessionCwd::parse(configured)
    } else {
        SessionCwd::parse(
            &Path::new(launch_cwd.as_str())
                .join(configured_path)
                .to_string_lossy(),
        )
    }
}
