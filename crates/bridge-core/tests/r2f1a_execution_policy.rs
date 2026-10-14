use bridge_core::diagnostics::{DiagnosticCode, DiagnosticFailureClass, DiagnosticRedactor};
use bridge_core::domain::{AgentEntry, AgentKind, EffectiveConfig, Effort, Part};
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{
    deadline_activation_v2_for, freeze_direct_checkout_v1, freeze_node_execution_identity_v1,
    freeze_provider_attempt_v1, freeze_worktree_checkout_v1, liveness_profile_v1,
    resolve_execution_policy_v1, resolve_execution_policy_with_readiness_v1,
    BoundMcpDeliveryPayloadV1, BoundSessionSpecV1, ControlEventIdV1, DeadlineActivationV2,
    ExecutionPolicyError, ExecutionPolicyInvocationV1, FanOutPolicyNameV1, FanOutPolicyV1,
    FrozenCheckoutEffectV1, FrozenProviderLogicalSessionV1, FrozenWorkflowControlsV1,
    LivenessProfileIdV1, NodeCauseV1, NodeCleanupDispositionV1, NodeCleanupV1,
    NodePrimaryDispositionV1, NodeTerminalV1, PolicyActivationV1, PolicyNodeRefV1, PolicyTriggerV1,
    ProfileSelectionSourceV1, ProviderEffectKeyV1, ProviderFreezeInputV1,
    SchedulerActivationReadinessV1, SilenceCutoffV1, SynthesisModeV1, TaskClassV1,
    WorkflowControlDefaultsV1, WorktreeCheckoutInputV1, MAX_NODE_TERMINAL_JSON_BYTES,
    MAX_POLICY_TRIGGER_JSON_BYTES, PROFILE_LEGACY_BOUNDED_V1, PROFILE_REVIEW_HIGH_XHIGH_V1,
};
use bridge_core::ids::{AgentId, AttemptId, SessionId};
use bridge_core::mcp::{McpDelivery, McpEnvValueSourceV1, McpServerSpec, SecretString};
use bridge_core::ports::{AgentBackend, BackendStream};
use bridge_core::turn_liveness::LivenessOverrideV1;
use bridge_core::SessionCwd;
use std::collections::BTreeMap;
use std::sync::Arc;

fn attempt() -> AttemptId {
    AttemptId::parse("attempt-11111111111111111111111111111111").unwrap()
}

#[test]
fn scheduler_readiness_and_policy_activation_select_deadline_activation() {
    use DeadlineActivationV2::{AutomaticR2f1b, ManualOnlyR2f1a};
    use PolicyActivationV1::{ManualTest, Production};
    use SchedulerActivationReadinessV1::{Armed, Disarmed};

    assert_eq!(
        deadline_activation_v2_for(Armed, Production),
        AutomaticR2f1b
    );
    let automatic_controls = resolve_execution_policy_with_readiness_v1(
        &WorkflowControlDefaultsV1::default(),
        &ExecutionPolicyInvocationV1::default(),
        false,
        Armed,
        Production,
    )
    .expect("automatic activation must be admitted by frozen-controls validation");
    assert_eq!(automatic_controls.deadline_activation, AutomaticR2f1b);
    for (readiness, policy) in [
        (Disarmed, Production),
        (Disarmed, ManualTest),
        (Armed, ManualTest),
    ] {
        assert_eq!(
            deadline_activation_v2_for(readiness, policy),
            ManualOnlyR2f1a
        );
    }
}

#[test]
fn manual_deadline_activation_controls_retain_literal_wire_bytes() {
    const LITERAL: &[u8] = br#"{"schema_version":1,"task_class":"other","profile":{"schema_version":1,"id":"legacy_bounded_v1","queue_wait_ms":1800000,"control_observable_ms":31000,"no_progress_snapshot_ms":1800000,"silence_cutoff":"none","work_cutoff_ms":7200000,"cancel_observable_ms":6000,"cleanup_tail_ms":60000,"reporting_tail_ms":10000,"terminal_bound_ms":7270000},"profile_source":"compatibility_omission","fan_out":{"kind":"bounded_independent"},"synthesis":"degraded","deadline_activation":"manual_only_r2f1a"}"#;

    let decoded: FrozenWorkflowControlsV1 =
        serde_json::from_slice(LITERAL).expect("persisted V1 manual controls must decode");
    assert_eq!(
        decoded.deadline_activation,
        DeadlineActivationV2::ManualOnlyR2f1a
    );
    assert_eq!(
        serde_json::to_vec(&decoded).expect("manual controls must encode"),
        LITERAL
    );
}

// ── Activity liveness I2a: the frozen workflow silence cutoff ────────────────────────────────

fn resolve_silence(
    workflow: Option<LivenessOverrideV1>,
    invocation: Option<LivenessOverrideV1>,
    readiness: SchedulerActivationReadinessV1,
    activation: PolicyActivationV1,
) -> Result<FrozenWorkflowControlsV1, ExecutionPolicyError> {
    resolve_execution_policy_with_readiness_v1(
        &WorkflowControlDefaultsV1 {
            silence_cutoff: workflow,
            ..WorkflowControlDefaultsV1::default()
        },
        &ExecutionPolicyInvocationV1 {
            silence_cutoff: invocation,
            ..ExecutionPolicyInvocationV1::default()
        },
        false,
        readiness,
        activation,
    )
}

/// Automatic activation inherits each node agent's silence cutoff; manual activation keeps none,
/// whatever the workflow or invocation asks for. The named profiles themselves are unchanged.
#[test]
fn automatic_resolve_freezes_agent_default_silence_and_manual_freezes_none() {
    use LivenessOverrideV1::{Ms, Off};
    use PolicyActivationV1::{ManualTest, Production};
    use SchedulerActivationReadinessV1::{Armed, Disarmed};

    for id in [
        LivenessProfileIdV1::LegacyBoundedV1,
        LivenessProfileIdV1::ReviewHighXhighV1,
    ] {
        assert_eq!(
            liveness_profile_v1(id).silence_cutoff,
            SilenceCutoffV1::None
        );
    }

    let automatic = resolve_execution_policy_v1(
        &WorkflowControlDefaultsV1::default(),
        &ExecutionPolicyInvocationV1::default(),
        false,
        Production,
    )
    .unwrap();
    assert_eq!(
        automatic.deadline_activation,
        DeadlineActivationV2::AutomaticR2f1b
    );
    assert_eq!(
        automatic.profile.silence_cutoff,
        SilenceCutoffV1::AgentDefault
    );

    for (readiness, activation) in [(Armed, ManualTest), (Disarmed, Production)] {
        for (workflow, invocation) in [
            (None, None),
            (Some(Ms(900_000)), None),
            (None, Some(Ms(900_000))),
            (Some(Off), None),
        ] {
            let manual = resolve_silence(workflow, invocation, readiness, activation).unwrap();
            assert_eq!(
                manual.deadline_activation,
                DeadlineActivationV2::ManualOnlyR2f1a
            );
            assert_eq!(
                manual.profile.silence_cutoff,
                SilenceCutoffV1::None,
                "manual activation keeps no silence cutoff ({workflow:?}, {invocation:?})"
            );
        }
    }
}

/// Invocation beats workflow beats the agent default; an explicit `off` at either level is kept
/// as `off` and never collapses into inherit. The workflow-wide 2 h work cutoff and its terminal
/// bound do not move whatever the silence value.
#[test]
fn silence_overrides_follow_invocation_then_workflow_and_keep_off_distinct() {
    use LivenessOverrideV1::{Ms, Off};

    let table = [
        (None, None, SilenceCutoffV1::AgentDefault),
        (Some(Off), None, SilenceCutoffV1::None),
        (
            Some(Ms(1_200_000)),
            None,
            SilenceCutoffV1::Activity {
                silence_ms: 1_200_000,
            },
        ),
        (None, Some(Off), SilenceCutoffV1::None),
        (
            None,
            Some(Ms(900_000)),
            SilenceCutoffV1::Activity {
                silence_ms: 900_000,
            },
        ),
        (
            Some(Off),
            Some(Ms(900_000)),
            SilenceCutoffV1::Activity {
                silence_ms: 900_000,
            },
        ),
        (Some(Ms(1_200_000)), Some(Off), SilenceCutoffV1::None),
        (
            Some(Ms(1_200_000)),
            Some(Ms(900_000)),
            SilenceCutoffV1::Activity {
                silence_ms: 900_000,
            },
        ),
    ];
    for (workflow, invocation, expected) in table {
        let controls = resolve_silence(
            workflow,
            invocation,
            SchedulerActivationReadinessV1::Armed,
            PolicyActivationV1::Production,
        )
        .unwrap();
        assert_eq!(
            controls.profile.silence_cutoff, expected,
            "workflow {workflow:?}, invocation {invocation:?}"
        );
        assert_eq!(controls.profile.work_cutoff_ms, 7_200_000);
        assert_eq!(controls.effective_work_cutoff_ms(), 7_200_000);
        assert_eq!(controls.effective_terminal_bound_ms().unwrap(), 7_270_000);
    }
}

/// A frozen override holds only what the shared parser can produce: whole seconds from 1 s to
/// 30 days. A carrier built by hand with anything else is refused, at either level.
#[test]
fn silence_override_values_outside_the_shared_parser_range_refuse() {
    use bridge_core::turn_liveness::{parse_liveness_secs_v1, MAX_LIVENESS_SECS};
    use LivenessOverrideV1::Ms;

    let thirty_days = parse_liveness_secs_v1(MAX_LIVENESS_SECS).unwrap();
    assert_eq!(thirty_days, Ms(2_592_000_000));
    assert!(parse_liveness_secs_v1(MAX_LIVENESS_SECS + 1).is_err());
    assert!(parse_liveness_secs_v1(u64::MAX).is_err());
    for level in 0..2 {
        let at = |value| {
            let (workflow, invocation) = if level == 0 {
                (Some(value), None)
            } else {
                (None, Some(value))
            };
            resolve_silence(
                workflow,
                invocation,
                SchedulerActivationReadinessV1::Armed,
                PolicyActivationV1::Production,
            )
        };
        assert_eq!(
            at(thirty_days).unwrap().profile.silence_cutoff,
            SilenceCutoffV1::Activity {
                silence_ms: 2_592_000_000
            }
        );
        for refused in [Ms(0), Ms(1_500), Ms(2_592_001_000), Ms(u64::MAX)] {
            assert_eq!(
                at(refused),
                Err(ExecutionPolicyError::InvalidSilenceCutoff),
                "{refused:?} at level {level}"
            );
        }
    }
}

/// A run frozen automatic before the workflow silence cutoff existed carries `"none"`. It still
/// decodes to no silence cutoff and re-encodes to the same bytes; the new values encode as
/// additive enum spellings inside the unchanged schema version.
#[test]
fn legacy_automatic_controls_literal_keeps_no_silence_cutoff_and_exact_bytes() {
    const LEGACY: &[u8] = br#"{"schema_version":1,"task_class":"other","profile":{"schema_version":1,"id":"legacy_bounded_v1","queue_wait_ms":1800000,"control_observable_ms":31000,"no_progress_snapshot_ms":1800000,"silence_cutoff":"none","work_cutoff_ms":7200000,"cancel_observable_ms":6000,"cleanup_tail_ms":60000,"reporting_tail_ms":10000,"terminal_bound_ms":7270000},"profile_source":"compatibility_omission","fan_out":{"kind":"bounded_independent"},"synthesis":"degraded","deadline_activation":"automatic_r2f1b"}"#;

    let legacy: FrozenWorkflowControlsV1 =
        serde_json::from_slice(LEGACY).expect("persisted automatic controls must decode");
    assert_eq!(
        legacy.deadline_activation,
        DeadlineActivationV2::AutomaticR2f1b
    );
    assert_eq!(legacy.profile.silence_cutoff, SilenceCutoffV1::None);
    assert_eq!(legacy.encode_canonical().unwrap(), LEGACY);
    assert_eq!(legacy.effective_work_cutoff_ms(), 7_200_000);

    let resolved = resolve_execution_policy_v1(
        &WorkflowControlDefaultsV1::default(),
        &ExecutionPolicyInvocationV1::default(),
        false,
        PolicyActivationV1::Production,
    )
    .unwrap();
    let as_frozen_now = String::from_utf8(LEGACY.to_vec()).unwrap().replace(
        r#""silence_cutoff":"none""#,
        r#""silence_cutoff":"agent_default""#,
    );
    assert_eq!(
        String::from_utf8(resolved.encode_canonical().unwrap()).unwrap(),
        as_frozen_now,
        "a new automatic run differs from the legacy row only in its silence value"
    );

    let mut activity = legacy.clone();
    activity.profile.silence_cutoff = SilenceCutoffV1::Activity {
        silence_ms: 900_000,
    };
    let encoded = activity.encode_canonical().unwrap();
    assert!(String::from_utf8(encoded.clone())
        .unwrap()
        .contains(r#""silence_cutoff":{"activity":{"silence_ms":900000}}"#));
    let decoded: FrozenWorkflowControlsV1 = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, activity);
    assert_eq!(decoded.encode_canonical().unwrap(), encoded);
}

/// The new override fields are absent from the encoding unless set, so persisted graphs and
/// invocations re-encode byte-identically; a set `off` round-trips as `off`, not as absence.
#[test]
fn silence_override_carriers_encode_only_when_set() {
    use LivenessOverrideV1::{Ms, Off};

    assert_eq!(
        serde_json::to_string(&WorkflowControlDefaultsV1::default()).unwrap(),
        "{}"
    );
    assert_eq!(
        serde_json::to_string(&ExecutionPolicyInvocationV1::default()).unwrap(),
        "{}"
    );
    for (value, json) in [
        (Off, r#"{"silence_cutoff":"off"}"#),
        (Ms(900_000), r#"{"silence_cutoff":{"ms":900000}}"#),
    ] {
        let defaults = WorkflowControlDefaultsV1 {
            silence_cutoff: Some(value),
            ..WorkflowControlDefaultsV1::default()
        };
        assert_eq!(serde_json::to_string(&defaults).unwrap(), json);
        assert_eq!(
            serde_json::from_str::<WorkflowControlDefaultsV1>(json).unwrap(),
            defaults
        );
        let invocation = ExecutionPolicyInvocationV1 {
            silence_cutoff: Some(value),
            ..ExecutionPolicyInvocationV1::default()
        };
        assert_eq!(serde_json::to_string(&invocation).unwrap(), json);
        assert_eq!(
            serde_json::from_str::<ExecutionPolicyInvocationV1>(json).unwrap(),
            invocation
        );
    }
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
fn inconsistent_profile_and_manual_fixed_grace_refuse() {
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
    let automatic = resolve_execution_policy_v1(
        &fixed,
        &ExecutionPolicyInvocationV1::default(),
        false,
        PolicyActivationV1::Production,
    )
    .unwrap();
    assert_eq!(
        automatic.deadline_activation,
        DeadlineActivationV2::AutomaticR2f1b
    );
    assert_eq!(
        resolve_execution_policy_v1(
            &fixed,
            &ExecutionPolicyInvocationV1::default(),
            false,
            PolicyActivationV1::ManualTest,
        ),
        Err(ExecutionPolicyError::FixedGraceInactive)
    );
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
        liveness: None,
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
        rename_new_sessions: false,
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

/// `rename_new_sessions` changes what the provider is sent before a turn, so it is part of the
/// frozen provider-effect identity. Only `true` is encoded: an entry that leaves it off keeps
/// exactly the digest it had before the field existed, or every identity a running serve has
/// already persisted would stop matching its own config across the upgrade.
///
/// The two literals were computed on the commit before the field was added (`1d7a5a68`), for
/// these same inputs, and pasted here: one per commitment form, the plain digest of an entry
/// with no MCP secret and the keyed one. A red on either means persisted provider-effect
/// identities no longer validate, which needs an explicit compatibility ruling.
#[test]
fn rename_new_sessions_joins_the_effect_digest_only_when_it_is_on() {
    let mut keyed_entry = provider_entry();
    keyed_entry.cmd = Some("codex-acp".into());
    let mut plain_entry = keyed_entry.clone();
    plain_entry.mcp.clear();
    let key = ProviderEffectKeyV1::from_bytes([9_u8; 32]);
    let worktree =
        freeze_worktree_checkout_v1(&checkout_input(FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        }))
        .unwrap();
    let direct = freeze_direct_checkout_v1(SessionCwd::parse("/repo").unwrap());

    for (entry, checkout, key, digest_before_the_field) in [
        (
            &plain_entry,
            direct,
            None,
            "804620bd39ceb58b6f5d329a36ed91a8aab7a033055bc7c505ebb7ceada0529f",
        ),
        (
            &keyed_entry,
            worktree,
            Some(&key),
            "acb2872723823babf3e46163e184e750d141586e09678b4d1fef111ccfe77f2f",
        ),
    ] {
        assert!(!entry.rename_new_sessions);
        let off = freeze_provider_attempt_v1(&provider_freeze_input(entry, checkout.clone(), key))
            .unwrap();
        assert_eq!(
            off.frozen.effect.effect_digest.as_str(),
            digest_before_the_field
        );

        let mut renaming_entry = entry.clone();
        renaming_entry.rename_new_sessions = true;
        let on = freeze_provider_attempt_v1(&provider_freeze_input(&renaming_entry, checkout, key))
            .unwrap();
        assert_ne!(
            off.frozen.effect.effect_digest,
            on.frozen.effect.effect_digest
        );
        assert_ne!(
            off.frozen.attempt_fingerprint,
            on.frozen.attempt_fingerprint
        );
        // Nothing else of the identity moves with the flag.
        assert_eq!(off.selection, on.selection);
        assert_eq!(off.frozen.checkout, on.frozen.checkout);
        assert_eq!(
            off.frozen.effect.mcp_delivery_digest,
            on.frozen.effect.mcp_delivery_digest
        );
    }
}

/// I1 test 13. `[agents.liveness]` bounds every turn the provider serves, so it is part of the
/// frozen provider-effect identity, encoded only when the entry sets it: an entry without it
/// keeps exactly the digest it had before the field existed. The literals are the same
/// pre-field digests `rename_new_sessions_joins_the_effect_digest_only_when_it_is_on` pins.
#[test]
fn agent_liveness_joins_the_effect_digest_only_when_it_is_set() {
    use bridge_core::domain::AgentLivenessConfig;
    use bridge_core::turn_liveness::LivenessOverrideV1;

    let mut keyed_entry = provider_entry();
    keyed_entry.cmd = Some("codex-acp".into());
    let mut plain_entry = keyed_entry.clone();
    plain_entry.mcp.clear();
    let key = ProviderEffectKeyV1::from_bytes([9_u8; 32]);
    let worktree =
        freeze_worktree_checkout_v1(&checkout_input(FrozenProviderLogicalSessionV1::Execute {
            candidate_ordinal: 0,
        }))
        .unwrap();
    let direct = freeze_direct_checkout_v1(SessionCwd::parse("/repo").unwrap());
    let configs = [
        AgentLivenessConfig::default(),
        AgentLivenessConfig {
            silence_cutoff: Some(LivenessOverrideV1::Off),
            work_cutoff: None,
        },
        AgentLivenessConfig {
            silence_cutoff: None,
            work_cutoff: Some(LivenessOverrideV1::Off),
        },
        AgentLivenessConfig {
            silence_cutoff: Some(LivenessOverrideV1::Ms(600_000)),
            work_cutoff: None,
        },
        AgentLivenessConfig {
            silence_cutoff: Some(LivenessOverrideV1::Ms(600_001)),
            work_cutoff: None,
        },
        AgentLivenessConfig {
            silence_cutoff: None,
            work_cutoff: Some(LivenessOverrideV1::Ms(600_000)),
        },
    ];

    for (entry, checkout, key, digest_before_the_field) in [
        (
            &plain_entry,
            direct,
            None,
            "804620bd39ceb58b6f5d329a36ed91a8aab7a033055bc7c505ebb7ceada0529f",
        ),
        (
            &keyed_entry,
            worktree,
            Some(&key),
            "acb2872723823babf3e46163e184e750d141586e09678b4d1fef111ccfe77f2f",
        ),
    ] {
        assert!(entry.liveness.is_none());
        let unset =
            freeze_provider_attempt_v1(&provider_freeze_input(entry, checkout.clone(), key))
                .unwrap();
        assert_eq!(
            unset.frozen.effect.effect_digest.as_str(),
            digest_before_the_field
        );

        let mut digests = vec![unset.frozen.effect.effect_digest.clone()];
        for config in configs {
            let mut configured = entry.clone();
            configured.liveness = Some(config);
            let set = freeze_provider_attempt_v1(&provider_freeze_input(
                &configured,
                checkout.clone(),
                key,
            ))
            .unwrap();
            // Nothing else of the identity moves with the table.
            assert_eq!(unset.selection, set.selection);
            assert_eq!(unset.frozen.checkout, set.frozen.checkout);
            assert_eq!(
                unset.frozen.effect.mcp_delivery_digest,
                set.frozen.effect.mcp_delivery_digest
            );
            digests.push(set.frozen.effect.effect_digest);
        }
        let distinct: std::collections::BTreeSet<_> =
            digests.iter().map(|digest| digest.as_str()).collect();
        assert_eq!(
            distinct.len(),
            digests.len(),
            "every set liveness value has its own digest"
        );
    }
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
