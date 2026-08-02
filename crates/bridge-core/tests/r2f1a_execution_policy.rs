use bridge_core::execution_policy::{
    freeze_worktree_checkout_v1, resolve_execution_policy_v1, ExecutionPolicyError,
    ExecutionPolicyInvocationV1, FanOutPolicyV1, FrozenCheckoutEffectV1,
    FrozenProviderLogicalSessionV1, LivenessProfileIdV1, PolicyActivationV1, PolicyNodeRefV1,
    ProfileSelectionSourceV1, SynthesisModeV1, TaskClassV1, WorkflowControlDefaultsV1,
    WorktreeCheckoutInputV1, PROFILE_LEGACY_BOUNDED_V1, PROFILE_REVIEW_HIGH_XHIGH_V1,
};
use bridge_core::ids::AttemptId;
use bridge_core::SessionCwd;

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
