use bridge_core::diagnostics::{DiagnosticCode, DiagnosticFailureClass, DiagnosticRedactor};
use bridge_core::domain::{AgentEntry, AgentKind, EffectiveConfig, Effort, Part};
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{
    freeze_direct_checkout_v1, freeze_node_execution_identity_v1, freeze_provider_attempt_v1,
    freeze_worktree_checkout_v1, resolve_execution_policy_v1, BoundMcpDeliveryPayloadV1,
    BoundSessionSpecV1, ControlEventIdV1, ExecutionPolicyError, ExecutionPolicyInvocationV1,
    FanOutPolicyNameV1, FanOutPolicyV1, FrozenCheckoutEffectV1, FrozenProviderLogicalSessionV1,
    LivenessProfileIdV1, NodeCauseV1, NodeCleanupDispositionV1, NodeCleanupV1,
    NodePrimaryDispositionV1, NodeTerminalV1, PolicyActivationV1, PolicyNodeRefV1, PolicyTriggerV1,
    ProfileSelectionSourceV1, ProviderEffectKeyV1, ProviderFreezeInputV1, SynthesisModeV1,
    TaskClassV1, WorkflowControlDefaultsV1, WorktreeCheckoutInputV1, MAX_NODE_TERMINAL_JSON_BYTES,
    MAX_POLICY_TRIGGER_JSON_BYTES, PROFILE_LEGACY_BOUNDED_V1, PROFILE_REVIEW_HIGH_XHIGH_V1,
};
use bridge_core::ids::{AgentId, AttemptId, SessionId};
use bridge_core::mcp::{McpDelivery, McpEnvValueSourceV1, McpServerSpec, SecretString};
use bridge_core::ports::{AgentBackend, BackendStream};
use bridge_core::SessionCwd;
use std::collections::BTreeMap;
use std::sync::Arc;

fn attempt() -> AttemptId {
    AttemptId::parse("attempt-11111111111111111111111111111111").unwrap()
}

#[test]
fn omission_and_invocation_precedence_freeze_exact_profiles() {
    let omitted = resolve_execution_policy_v1(
        &WorkflowControlDefaultsV1::default(),
        &ExecutionPolicyInvocationV1::default(),
        false,
        PolicyActivationV1::Production,
    )
    .unwrap();
    assert_eq!(omitted.task_class, TaskClassV1::Other);
    assert_eq!(omitted.profile.id.as_str(), PROFILE_LEGACY_BOUNDED_V1);
    assert_eq!(
        omitted.profile_source,
        ProfileSelectionSourceV1::CompatibilityOmission
    );
    assert_eq!(omitted.fan_out, FanOutPolicyV1::BoundedIndependent);
    assert_eq!(omitted.synthesis, SynthesisModeV1::Degraded);
    assert_eq!(omitted.effective_work_cutoff_ms(), 7_200_000);
    assert_eq!(omitted.effective_terminal_bound_ms().unwrap(), 7_270_000);

    let workflow = WorkflowControlDefaultsV1 {
        task_class: Some(TaskClassV1::Other),
        liveness_profile: Some(LivenessProfileIdV1::LegacyBoundedV1),
        ..Default::default()
    };
    let invocation = ExecutionPolicyInvocationV1 {
        liveness_profile: Some(LivenessProfileIdV1::ReviewHighXhighV1),
        ..Default::default()
    };
    let overridden = resolve_execution_policy_v1(
        &workflow,
        &invocation,
        false,
        PolicyActivationV1::Production,
    )
    .unwrap();
    assert_eq!(overridden.profile.id.as_str(), PROFILE_REVIEW_HIGH_XHIGH_V1);
    assert_eq!(
        overridden.profile_source,
        ProfileSelectionSourceV1::Invocation
    );
    assert_eq!(overridden.task_class, TaskClassV1::ReviewHighXhigh);
}

#[test]
fn inconsistent_profile_and_inactive_fixed_grace_refuse() {
    let mismatch = WorkflowControlDefaultsV1 {
        task_class: Some(TaskClassV1::Other),
        liveness_profile: Some(LivenessProfileIdV1::ReviewHighXhighV1),
        ..Default::default()
    };
    assert_eq!(
        resolve_execution_policy_v1(
            &mismatch,
            &ExecutionPolicyInvocationV1::default(),
            false,
            PolicyActivationV1::Production,
        ),
        Err(ExecutionPolicyError::ProfileTaskClassMismatch)
    );

    let fixed = WorkflowControlDefaultsV1 {
        fan_out: Some(FanOutPolicyV1::FixedGrace { grace_ms: 30_000 }),
        ..Default::default()
    };
    assert_eq!(
        resolve_execution_policy_v1(
            &fixed,
            &ExecutionPolicyInvocationV1::default(),
            false,
            PolicyActivationV1::Production,
        ),
        Err(ExecutionPolicyError::FixedGraceInactive)
    );
    assert!(resolve_execution_policy_v1(
        &fixed,
        &ExecutionPolicyInvocationV1::default(),
        false,
        PolicyActivationV1::ManualTest,
    )
    .is_ok());
}

#[test]
fn max_pair_is_atomic_bounded_and_changes_effective_terminal_bound() {
    let invocation = ExecutionPolicyInvocationV1 {
        max_work_cutoff_ms: Some(7_200_001),
        max_reason: Some("one bounded max-effort proof".into()),
        ..Default::default()
    };
    let controls = resolve_execution_policy_v1(
        &WorkflowControlDefaultsV1::default(),
        &invocation,
        true,
        PolicyActivationV1::Production,
    )
    .unwrap();
    assert_eq!(controls.effective_work_cutoff_ms(), 7_200_001);
    assert_eq!(controls.effective_terminal_bound_ms().unwrap(), 7_270_001);

    let partial = ExecutionPolicyInvocationV1 {
        max_work_cutoff_ms: Some(7_200_001),
        ..Default::default()
    };
    assert_eq!(
        resolve_execution_policy_v1(
            &WorkflowControlDefaultsV1::default(),
            &partial,
            true,
            PolicyActivationV1::Production,
        ),
        Err(ExecutionPolicyError::PartialMaxQualification)
    );

    assert_eq!(
        resolve_execution_policy_v1(
            &WorkflowControlDefaultsV1::default(),
            &invocation,
            false,
            PolicyActivationV1::Production,
        ),
        Err(ExecutionPolicyError::UnusedMaxQualification)
    );
}

fn checkout_input(logical_session: FrozenProviderLogicalSessionV1) -> WorktreeCheckoutInputV1 {
    WorktreeCheckoutInputV1 {
        attempt_id: attempt(),
        node: PolicyNodeRefV1::from_node_id(3, "review-node"),
        logical_session,
        source_cwd: SessionCwd::parse("/allowed/link/repo").unwrap(),
        canonical_source_cwd: SessionCwd::parse("/allowed/real/repo").unwrap(),
        canonical_worktree_root: SessionCwd::parse("/state/a2a/worktrees").unwrap(),
        worktree_owner: "owner-7".into(),
    }
}

#[test]
fn persisted_logical_session_derives_exact_restart_stable_worktree_targets() {
    let execute =
        freeze_worktree_checkout_v1(&checkout_input(FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        }))
        .unwrap();
    let restarted =
        freeze_worktree_checkout_v1(&checkout_input(FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        }))
        .unwrap();
    assert_eq!(execute, restarted);

    let preflight =
        freeze_worktree_checkout_v1(&checkout_input(FrozenProviderLogicalSessionV1::Preflight {
            candidate_ordinal: 0,
        }))
        .unwrap();
    let other_candidate =
        freeze_worktree_checkout_v1(&checkout_input(FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 1,
        }))
        .unwrap();
    assert_ne!(execute, preflight);
    assert_ne!(execute, other_candidate);

    let FrozenCheckoutEffectV1::Worktree {
        target_cwd,
        canonical_worktree_root,
        ..
    } = &execute
    else {
        panic!("worktree freeze must produce a worktree checkout")
    };
    assert!(target_cwd.is_under(canonical_worktree_root));
    assert!(target_cwd.as_str().contains("owner-7-r2f1a-"));

    let encoded = serde_json::to_vec(&execute).unwrap();
    let decoded: FrozenCheckoutEffectV1 = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, execute);
    assert!(!String::from_utf8(encoded)
        .unwrap()
        .contains("run.instance_id"));
}

#[test]
fn unsafe_worktree_owner_refuses_before_a_target_exists() {
    let mut input = checkout_input(FrozenProviderLogicalSessionV1::Execute {
        candidate_ordinal: 0,
    });
    input.worktree_owner = "../escape".into();
    assert_eq!(
        freeze_worktree_checkout_v1(&input),
        Err(ExecutionPolicyError::InvalidWorktreeOwner)
    );
}

#[test]
fn persisted_session_cwd_decode_reuses_parse_invariants() {
    assert!(serde_json::from_str::<SessionCwd>(r#""relative/path""#).is_err());
    assert!(serde_json::from_str::<SessionCwd>(r#""/a/../..""#).is_err());
    let decoded: SessionCwd = serde_json::from_str(r#""/a/./b""#).unwrap();
    assert_eq!(decoded.as_str(), "/a/b");
}

fn provider_entry() -> AgentEntry {
    AgentEntry {
        id: AgentId::parse("reviewer").unwrap(),
        cmd: Some("claude-agent-acp".into()),
        base_url: None,
        api_key_env: None,
        args: vec!["--stdio".into()],
        kind: AgentKind::Acp,
        model_provider: None,
        model: Some("primary".into()),
        effort: Some(Effort::Xhigh),
        mode: Some("read-only".into()),
        preflight: true,
        fallback_models: vec!["fallback-a".into(), "fallback-b".into()],
        cwd: None,
        session_cwd: None,
        sandbox: None,
        watchdog: None,
        mcp: vec![McpServerSpec {
            name: "prism".into(),
            command: "/opt/prism".into(),
            args: vec!["--repo".into(), "{cwd}".into()],
            env: vec![
                ("ROOT".into(), "{cwd}/src".into()),
                ("PIN".into(), "0042".into()),
            ],
        }],
        mcp_delivery: McpDelivery::Acp,
        auth_method: None,
        pre_authenticated: true,
        host_fallback_eligible: false,
        name: Some("reader".into()),
        description: None,
        tags: vec![],
        version: None,
        extensions: BTreeMap::new(),
    }
}

#[test]
fn node_identity_requires_the_complete_canonical_logical_session_matrix() {
    let mut entry = provider_entry();
    entry.fallback_models = vec!["fallback-a".into()];
    let node = PolicyNodeRefV1::from_node_id(0, "review-node");
    let checkout = freeze_direct_checkout_v1(SessionCwd::parse("/repo").unwrap());
    let key = ProviderEffectKeyV1::from_bytes([17; 32]);
    let mut attempts = Vec::new();
    for ordinal in 0..=1 {
        for logical_session in [
            FrozenProviderLogicalSessionV1::Preflight {
                candidate_ordinal: ordinal,
            },
            FrozenProviderLogicalSessionV1::Execute {
                candidate_ordinal: ordinal,
            },
        ] {
            attempts.push(
                freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
                    entry: &entry,
                    overrides: None,
                    node: node.clone(),
                    logical_session,
                    checkout: checkout.clone(),
                    provider_effect_key: Some(&key),
                })
                .unwrap(),
            );
        }
    }

    let identity = freeze_node_execution_identity_v1(node.clone(), attempts.clone()).unwrap();
    assert_eq!(identity.provider_attempts.len(), 4);
    assert_eq!(identity.node, node);

    attempts.remove(2);
    assert_eq!(
        freeze_node_execution_identity_v1(identity.node.clone(), attempts),
        Err(ExecutionPolicyError::ProviderAttemptMatrixInvalid)
    );
}

#[test]
fn preflight_disabled_identity_has_one_execute_row_and_no_retry_rows() {
    let mut entry = provider_entry();
    entry.preflight = false;
    entry.fallback_models.clear();
    let node = PolicyNodeRefV1::from_node_id(0, "review-node");
    let bundle = freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
        entry: &entry,
        overrides: None,
        node: node.clone(),
        logical_session: FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        },
        checkout: freeze_direct_checkout_v1(SessionCwd::parse("/repo").unwrap()),
        provider_effect_key: Some(&ProviderEffectKeyV1::from_bytes([18; 32])),
    })
    .unwrap();
    let identity = freeze_node_execution_identity_v1(node, vec![bundle]).unwrap();
    assert_eq!(identity.provider_attempts.len(), 1);
    assert!(matches!(
        identity.provider_attempts[0].logical_session,
        FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0
        }
    ));
}

#[test]
fn terminal_canonical_encoding_bounds_escape_expansion_and_keeps_failure_identity() {
    let cause_text = format!("🌞 deepest {}", "\\\"".repeat(260));
    let terminal = NodeTerminalV1 {
        schema_version: 1,
        primary: NodePrimaryDispositionV1::Failed,
        cleanup: NodeCleanupV1 {
            disposition: NodeCleanupDispositionV1::Failed,
            duration_ms: u64::MAX,
        },
        cause: Some(NodeCauseV1 {
            failure_class: DiagnosticFailureClass::ContainerCredentials,
            code: DiagnosticCode::build("container.credentials", &DiagnosticRedactor::default())
                .unwrap(),
            deepest_cause: Some(cause_text),
            cause_truncated: false,
            evidence_overflow: false,
            dependency_set: None,
        }),
        prompt_may_have_been_accepted: true,
        degraded_ancestry: true,
        policy_trigger_id: None,
    };
    let encoded = terminal.encode_canonical().unwrap();
    assert!(encoded.len() <= MAX_NODE_TERMINAL_JSON_BYTES);
    assert!(std::str::from_utf8(&encoded).unwrap().contains("🌞"));
    assert!(!std::str::from_utf8(&encoded).unwrap().contains("\\ud83c"));
    let decoded = NodeTerminalV1::decode_canonical(&encoded).unwrap();
    let cause = decoded.cause.unwrap();
    assert_eq!(
        cause.failure_class,
        DiagnosticFailureClass::ContainerCredentials
    );
    assert_eq!(cause.code.as_str(), "container.credentials");
    assert!(cause.cause_truncated);
}

#[test]
fn policy_trigger_has_a_bounded_validated_control_identity() {
    let trigger = PolicyTriggerV1 {
        schema_version: 1,
        id: ControlEventIdV1::parse("attempt-11111111111111111111111111111111:policy:0").unwrap(),
        node: PolicyNodeRefV1::from_node_id(0, "review-node"),
        policy: FanOutPolicyNameV1::FailFast,
        grace_ms: None,
    };
    let encoded = trigger.encode_canonical().unwrap();
    assert!(encoded.len() <= MAX_POLICY_TRIGGER_JSON_BYTES);
    assert_eq!(
        PolicyTriggerV1::decode_canonical(&encoded).unwrap(),
        trigger
    );
    assert!(ControlEventIdV1::parse("x".repeat(129)).is_err());
}

#[test]
fn typed_mcp_secret_source_is_immutable_and_secret_silent() {
    let secret = McpEnvValueSourceV1::SecretFromEnv {
        variable: "TEST_MCP_SECRET".into(),
        resolved: SecretString::new("alpha{cwd}omega").unwrap(),
    };
    let debug = format!("{secret:?}");
    assert!(debug.contains("TEST_MCP_SECRET"));
    assert!(!debug.contains("alpha"));
    assert_eq!(secret.delivery_value("/repo"), "alpha{cwd}omega");

    let public = McpEnvValueSourceV1::from("alpha{cwd}omega");
    assert_eq!(public.delivery_value("/repo"), "alpha/repoomega");
}

fn provider_freeze_input<'a>(
    entry: &'a AgentEntry,
    checkout: FrozenCheckoutEffectV1,
    key: Option<&'a ProviderEffectKeyV1>,
) -> ProviderFreezeInputV1<'a> {
    ProviderFreezeInputV1 {
        entry,
        overrides: None,
        node: PolicyNodeRefV1::from_node_id(3, "review-node"),
        logical_session: FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        },
        checkout,
        provider_effect_key: key,
    }
}

#[test]
fn bound_provider_attempt_commits_and_delivers_the_worktree_target_without_secret_oracle() {
    let entry = provider_entry();
    let checkout =
        freeze_worktree_checkout_v1(&checkout_input(FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        }))
        .unwrap();
    assert!(matches!(
        freeze_provider_attempt_v1(&provider_freeze_input(&entry, checkout.clone(), None)),
        Err(ExecutionPolicyError::MissingProviderEffectKey)
    ));

    let key = ProviderEffectKeyV1::from_bytes([7_u8; 32]);
    let bundle =
        freeze_provider_attempt_v1(&provider_freeze_input(&entry, checkout.clone(), Some(&key)))
            .unwrap();
    assert_eq!(
        bundle.frozen.effect.effective_session_cwd,
        checkout.effective_cwd().clone()
    );
    assert!(bundle.frozen.effect.secret_commitment_key_id.is_some());
    let durable = serde_json::to_string(&bundle.frozen).unwrap();
    assert!(!durable.contains("0042"), "{durable}");
    assert!(!format!("{:?}", bundle.bound).contains("0042"));

    let BoundMcpDeliveryPayloadV1::Acp(servers) = bundle.bound.delivery().payload() else {
        panic!("ACP entry must freeze ACP delivery")
    };
    assert_eq!(servers[0].args[1], checkout.effective_cwd().as_str());
    assert_eq!(
        servers[0].env[0].1.resolved_value(),
        format!("{}/src", checkout.effective_cwd().as_str())
    );
    assert_eq!(servers[0].env[1].1.resolved_value(), "0042");
}

#[test]
fn provider_identity_separates_selection_order_effect_changes_and_presentation_exclusions() {
    let key = ProviderEffectKeyV1::from_bytes([9_u8; 32]);
    let checkout =
        freeze_worktree_checkout_v1(&checkout_input(FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        }))
        .unwrap();
    let base_entry = provider_entry();
    let base = freeze_provider_attempt_v1(&provider_freeze_input(
        &base_entry,
        checkout.clone(),
        Some(&key),
    ))
    .unwrap();

    let mut reordered_entry = provider_entry();
    reordered_entry.fallback_models.swap(0, 1);
    let reordered = freeze_provider_attempt_v1(&provider_freeze_input(
        &reordered_entry,
        checkout.clone(),
        Some(&key),
    ))
    .unwrap();
    assert_ne!(
        base.selection.selection_digest,
        reordered.selection.selection_digest
    );
    assert_eq!(
        base.frozen.effect.effect_digest,
        reordered.frozen.effect.effect_digest
    );

    let mut changed_effect_entry = provider_entry();
    changed_effect_entry.cmd = Some("other-acp".into());
    let changed_effect = freeze_provider_attempt_v1(&provider_freeze_input(
        &changed_effect_entry,
        checkout.clone(),
        Some(&key),
    ))
    .unwrap();
    assert_ne!(
        base.frozen.effect.effect_digest,
        changed_effect.frozen.effect.effect_digest
    );

    let mut presentation_entry = provider_entry();
    presentation_entry.name = Some("renamed".into());
    presentation_entry.tags.push("new-tag".into());
    presentation_entry
        .extensions
        .insert("ui".into(), toml::Value::Boolean(true));
    let presentation = freeze_provider_attempt_v1(&provider_freeze_input(
        &presentation_entry,
        checkout,
        Some(&key),
    ))
    .unwrap();
    assert_eq!(base.selection, presentation.selection);
    assert_eq!(base.frozen, presentation.frozen);
}

struct LegacyBackend;

#[async_trait::async_trait]
impl AgentBackend for LegacyBackend {
    async fn prompt(
        &self,
        _session: &SessionId,
        _parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        unreachable!("bound-session contract test never prompts")
    }

    async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}

#[tokio::test]
async fn bound_session_is_exact_and_legacy_backend_refuses_without_fallback() {
    let key = ProviderEffectKeyV1::from_bytes([5_u8; 32]);
    let entry = provider_entry();
    let checkout =
        freeze_worktree_checkout_v1(&checkout_input(FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        }))
        .unwrap();
    let bundle =
        freeze_provider_attempt_v1(&provider_freeze_input(&entry, checkout.clone(), Some(&key)))
            .unwrap();
    let bound = BoundSessionSpecV1::new(EffectiveConfig::default(), Arc::new(bundle.bound));
    assert_eq!(bound.session.cwd.as_ref(), Some(checkout.effective_cwd()));
    assert_eq!(
        LegacyBackend
            .configure_bound_session(&SessionId::parse("bound-session").unwrap(), &bound)
            .await,
        Err(BridgeError::BoundSessionUnsupported)
    );
}
