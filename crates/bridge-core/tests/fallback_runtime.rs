use bridge_core::diagnostics::DiagnosticFailureClass as Class;
use bridge_core::ids::AgentId;
use bridge_core::routing::*;

fn candidate(agent: &str) -> FallbackCandidateV1 {
    FallbackCandidateV1 {
        agent: AgentId::parse(agent).unwrap(),
        model: agent.into(),
        posture: CandidatePostureV1 {
            kind: "acp".into(),
            cmd: Some("synthetic-read-only-adapter".into()),
            ..Default::default()
        },
    }
}
fn chain() -> FrozenFallbackChainV1 {
    FrozenFallbackChainV1 {
        id: "capacity".into(),
        source: candidate("source"),
        causes: [FallbackCauseV1::QuotaExhausted].into(),
        steps: vec![candidate("target"), candidate("last")],
    }
}
#[test]
fn nonmatching_last_failure_stays_original() {
    assert_eq!(
        decide_fallback(
            Some(&chain()),
            Class::BillingFailure,
            ReplayEvidenceV1::default(),
            FallbackPositionV1::Step(1)
        ),
        RoutingDecisionV1::OriginalFailure
    );
}

use bridge_core::diagnostics::*;
use bridge_core::error::BridgeError;
use bridge_core::fallback_runtime::FrozenFallbackInvocationV1 as Invocation;
use bridge_core::ports::BackendCleanupDispositionV1 as Cleanup;
use bridge_core::translator::AccountingFailureV1;

fn failure(class: Class) -> AccountingFailureV1 {
    let diagnostic = FailureDiagnostic::build(
        FailureDiagnosticInput {
            failed_phase: DiagnosticPhase::PromptStart,
            last_completed_phase: None,
            class,
            disposition: FailureDisposition::Fatal,
            code: "upstream.rejected".into(),
            summary: "original".into(),
            causes: vec![],
            stderr_observed: false,
            stderr_line_count: 0,
            stderr_scope: None,
            stderr_tail: None,
            stderr_redaction: None,
            retry_after_ms: None,
            reset_at_ms: None,
            prompt_may_have_been_accepted: false,
        },
        &DiagnosticRedactor::new([] as [&str; 0]),
    )
    .unwrap();
    let mut failure = AccountingFailureV1::from(BridgeError::AgentFailure {
        diagnostic: Box::new(diagnostic),
    });
    failure.replay = ReplayEvidenceV1 {
        prompt_acceptance: ExclusionEvidenceV1::Excluded,
        effects: ExclusionEvidenceV1::Excluded,
        cleanup_completed: true,
    };
    failure.cleanup = Cleanup::Complete;
    failure
}
fn scoped_chain(path: &std::path::Path) -> FrozenFallbackChainV1 {
    let mut chain = chain();
    chain.source.posture.cwd = path.display().to_string();
    chain.source.posture.session_cwd = path.display().to_string();
    for step in &mut chain.steps {
        step.posture = chain.source.posture.clone();
    }
    chain
}
fn invocation(chain: FrozenFallbackChainV1) -> Invocation {
    Invocation::freeze(vec![chain.clone()], chain.source).unwrap()
}
fn refuse(run: Invocation, failure: AccountingFailureV1, canceled: bool) {
    let expected = failure.clone();
    let Err(actual) = run.transition(failure, canceled, |_| panic!("ineligible target prepared"))
    else {
        panic!("advanced")
    };
    assert_eq!(actual, expected);
}
#[test]
fn eligibility_matrix_is_closed_and_lazy() {
    let dir = tempfile::tempdir().unwrap();
    let chain = scoped_chain(dir.path());
    for class in Class::ALL {
        let mut policy = chain.clone();
        if let Ok(cause) = FallbackCauseV1::try_from(class) {
            policy.causes = [cause].into();
        }
        if FallbackCauseV1::try_from(class).is_ok() {
            let (_, selected) = invocation(policy)
                .transition(failure(class), false, |next| Ok(next.clone()))
                .ok()
                .unwrap();
            assert_eq!(selected, chain.steps[0]);
        } else {
            refuse(invocation(policy), failure(class), false);
        }
    }
    for excluded_field in [true, false] {
        for evidence in [ExclusionEvidenceV1::Unknown, ExclusionEvidenceV1::Present] {
            let mut unsafe_failure = failure(Class::QuotaExhausted);
            if excluded_field {
                unsafe_failure.replay.prompt_acceptance = evidence;
            } else {
                unsafe_failure.replay.effects = evidence;
            }
            // Accepted/unknown prompt, partial text, tool and permission effects.
            refuse(invocation(chain.clone()), unsafe_failure, false);
        }
    }
    for cleanup in [Cleanup::Unknown, Cleanup::Retained, Cleanup::Preserved] {
        let mut f = failure(Class::QuotaExhausted);
        f.cleanup = cleanup;
        refuse(invocation(chain.clone()), f, false);
    }
    let mut f = failure(Class::QuotaExhausted);
    f.replay.cleanup_completed = false;
    refuse(invocation(chain.clone()), f, false);
    let mut f = failure(Class::QuotaExhausted);
    f.persistence = Some(BridgeError::StoreFailure);
    refuse(invocation(chain.clone()), f, false);
    let mut f = failure(Class::QuotaExhausted);
    f.original = BridgeError::MissingTerminal;
    refuse(invocation(chain.clone()), f, false);
    refuse(
        invocation(chain.clone()),
        failure(Class::QuotaExhausted),
        true,
    );
    refuse(
        invocation(chain.clone()),
        failure(Class::BillingFailure),
        false,
    );
    let mut f = failure(Class::QuotaExhausted);
    f.original = BridgeError::BudgetRefused {
        refusal: Box::new(bridge_core::governance::BudgetRefusalV1::new(
            bridge_core::governance::BudgetRefusalReasonV1::MissingAllowance,
        )),
    };
    refuse(invocation(chain.clone()), f, false);
    let source = chain.source.clone();
    refuse(
        Invocation::freeze(vec![], source.clone()).unwrap(),
        failure(Class::QuotaExhausted),
        false,
    );
    refuse(
        Invocation::freeze(vec![chain.clone(), chain], source).unwrap(),
        failure(Class::QuotaExhausted),
        false,
    );
}
#[test]
fn sequence_never_reroots_and_preserves_later_failures() {
    let dir = tempfile::tempdir().unwrap();
    let chain = scoped_chain(dir.path());
    let mut reroot = chain.clone();
    reroot.source = chain.steps[0].clone();
    let run = Invocation::freeze(vec![chain.clone(), reroot], chain.source.clone()).unwrap();
    let (run, first) = run
        .transition(failure(Class::QuotaExhausted), false, |next| {
            Ok(next.clone())
        })
        .ok()
        .unwrap();
    assert_eq!(first, chain.steps[0]);
    let (run, last) = run
        .transition(failure(Class::QuotaExhausted), false, |next| {
            Ok(next.clone())
        })
        .ok()
        .unwrap();
    assert_eq!(last, chain.steps[1]);
    refuse(run, failure(Class::BillingFailure), false);
    let mut run = invocation(chain);
    for _ in 0..2 {
        run = run
            .transition(failure(Class::QuotaExhausted), false, |next| {
                Ok(next.clone())
            })
            .ok()
            .unwrap()
            .0;
    }
    refuse(run, failure(Class::QuotaExhausted), false);
}
#[test]
fn effective_authority_and_selector_must_match_before_execution() {
    let dir = tempfile::tempdir().unwrap();
    let chain = scoped_chain(dir.path());
    for field in 0..8 {
        let expected = failure(Class::QuotaExhausted);
        let result = invocation(chain.clone()).transition(expected.clone(), false, |next| {
            let mut effective = next.clone();
            match field {
                0 => effective.posture.kind = "host".into(),
                1 => effective.posture.cwd = "/".into(),
                2 => effective.posture.session_cwd = "/".into(),
                3 => effective.posture.mode = Some("write".into()),
                4 => effective.posture.args.push("--extra-tools".into()),
                5 => effective.posture.mcp_delivery = "extra".into(),
                6 => effective.model = "undeclared".into(),
                _ => return Err(BridgeError::ModelNotAvailable),
            }
            Ok(effective)
        });
        assert!(matches!(result, Err(actual) if actual == expected));
    }
    let mut malformed = chain.clone();
    malformed.steps[0].posture.args.push("--write".into());
    refuse(invocation(malformed), failure(Class::QuotaExhausted), false);
    let mut source = chain.source.clone();
    source.posture.mode = Some("override".into());
    refuse(
        Invocation::freeze(vec![chain], source).unwrap(),
        failure(Class::QuotaExhausted),
        false,
    );
}
#[test]
fn changed_scope_object_and_unprovable_scope_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    std::fs::create_dir(&path).unwrap();
    let chain = scoped_chain(&path);
    let run = invocation(chain.clone());
    std::fs::rename(&path, dir.path().join("old")).unwrap();
    std::fs::create_dir(&path).unwrap();
    refuse(run, failure(Class::QuotaExhausted), false);
    let mut source = chain.source;
    source.posture.cwd = dir.path().join("missing").display().to_string();
    assert!(Invocation::freeze(vec![], source).is_none());
}

#[test]
fn sandbox_mount_write_and_mcp_authority_are_equal_or_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut chain = scoped_chain(dir.path());
    chain.source.posture.sandbox = Some(SandboxPostureV1 {
        runtime: "docker".into(),
        image: "reader:fixed".into(),
        mount: dir.path().display().to_string(),
        access: "ro".into(),
        egress: "locked".into(),
        network: Some("fixed".into()),
        proxy: None,
        no_proxy: None,
        volumes: vec![],
    });
    for step in &mut chain.steps {
        step.posture = chain.source.posture.clone();
    }
    assert!(invocation(chain.clone())
        .transition(failure(Class::QuotaExhausted), false, |c| Ok(c.clone()))
        .is_ok());
    for field in 0..4 {
        let mut policy = chain.clone();
        let posture = &mut policy.steps[0].posture;
        match field {
            0 => posture.sandbox = None,
            1 => posture.sandbox.as_mut().unwrap().mount = "/".into(),
            2 => posture.sandbox.as_mut().unwrap().access = "rw".into(),
            _ => {
                posture.mcp.insert(McpPostureV1 {
                    name: "extra".into(),
                    command: "tools".into(),
                    args: vec![],
                    env: Default::default(),
                });
            }
        }
        refuse(invocation(policy), failure(Class::QuotaExhausted), false);
    }
}
