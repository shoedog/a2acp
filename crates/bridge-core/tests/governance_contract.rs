use bridge_core::{
    diagnostics::DiagnosticFailureClass, error::BridgeError, governance::*, ids::AgentId,
    orch::UsageSnapshot,
};
use std::num::NonZeroU64;
use BudgetLiabilityV1::{Known, Pending, Unknown};
fn candidate(model: Option<&str>) -> BudgetCandidateV1 {
    BudgetCandidateV1 {
        agent: AgentId::parse("a").unwrap(),
        model: model.map(str::to_owned),
    }
}
fn rule(id: &str, selector: BudgetSelectorV1, tokens: bool) -> BudgetRuleV1 {
    BudgetRuleV1 {
        id: id.into(),
        selector,
        limits: BudgetLimitsV1 {
            turns: Some(0),
            terminal_tokens: tokens.then_some(100),
        },
    }
}
fn usage(total: u64) -> UsageSnapshot {
    let terminal = serde_json::from_value(serde_json::json!({
        "totalTokens": total, "inputTokens": 999, "outputTokens": 999
    }))
    .unwrap();
    UsageSnapshot {
        terminal: Some(terminal),
        ..Default::default()
    }
}
#[test]
fn exact_allowances_and_applicability() {
    let rules = vec![rule("r", BudgetSelectorV1::EachRequest, true)];
    let allowances = [None, Some("m1"), Some("m2")]
        .into_iter()
        .enumerate()
        .map(|(i, m)| TurnAllowanceV1 {
            target: candidate(m),
            terminal_tokens: NonZeroU64::new(100 + i as u64).unwrap(),
        })
        .collect();
    let p = BudgetPolicyV1::new(rules.clone(), allowances).unwrap();
    for (model, expected) in [(None, 100), (Some("m1"), 101), (Some("m2"), 102)] {
        let amount = p.allowance_for(&candidate(model), false).unwrap();
        assert_eq!(amount.map(NonZeroU64::get), Some(expected));
    }
    assert_eq!(
        p.allowance_for(&candidate(Some("other")), false)
            .unwrap_err()
            .reason,
        BudgetRefusalReasonV1::MissingAllowance
    );
    let missing = BudgetPolicyV1::new(rules, vec![]).unwrap();
    assert!(missing.allowance_for(&candidate(None), true).is_err());
    for selector in [
        BudgetSelectorV1::EachRequest,
        BudgetSelectorV1::EachWorkflowRun,
        BudgetSelectorV1::UtcDayAgent(AgentId::parse("b").unwrap()),
    ] {
        let p = BudgetPolicyV1::new(vec![rule("r", selector.clone(), false)], vec![]).unwrap();
        assert_eq!(p.allowance_for(&candidate(None), true).unwrap(), None);
        let p = BudgetPolicyV1::new(vec![rule("r", selector.clone(), true)], vec![]).unwrap();
        assert_eq!(
            p.allowance_for(&candidate(None), false).is_err(),
            selector == BudgetSelectorV1::EachRequest
        );
    }
}
#[test]
fn pending_high_water_and_final_actuals() {
    let pending =
        Pending(100)
            .reconcile(&usage(120), false, false)
            .reconcile(&usage(80), false, false);
    assert_eq!(pending, Pending(120));
    assert_eq!(pending.reconcile(&usage(80), true, false), Known(80));
    assert_eq!(pending.reconcile(&usage(150), true, false), Known(150));
    let context_only = UsageSnapshot {
        used: Some(300),
        size: Some(500),
        ..Default::default()
    };
    let unknown = |reason| Unknown {
        retained: 120,
        reason,
    };
    assert_eq!(
        pending.reconcile(&context_only, true, false),
        unknown(BudgetUnknownReasonV1::MissingTerminalTokens)
    );
    assert_eq!(
        pending.reconcile(&usage(80), true, true),
        unknown(BudgetUnknownReasonV1::InvalidTerminalTokens)
    );
    assert_eq!(pending.reconcile(&usage(0), true, false), Known(0));
}
#[test]
fn canonical_policy_and_validation() {
    let a = rule("a", BudgetSelectorV1::EachRequest, false);
    let b = rule("b", BudgetSelectorV1::EachWorkflowRun, false);
    let digest = |rules| BudgetPolicyV1::new(rules, vec![]).unwrap().digest();
    let original = digest(vec![a.clone(), b.clone()]);
    assert_eq!(original, digest(vec![b.clone(), a.clone()]));
    let mut changed = a.clone();
    changed.limits.turns = Some(1);
    assert_ne!(original, digest(vec![changed, b]));
    assert!(BudgetPolicyV1::new(vec![a.clone(), a], vec![]).is_err());
    assert!(BudgetPolicyV1::new(vec![], vec![]).is_err());
}
#[test]
fn refusal_is_static_and_non_transient() {
    let refusal = BudgetRefusalV1 {
        reason: BudgetRefusalReasonV1::LimitExceeded,
        scope: Some(BudgetScopeKeyV1 {
            rule_id: "private-budget".into(),
            owner: BudgetScopeOwnerV1::UtcDayAgent {
                agent: candidate(None).agent,
                utc_day: 999,
            },
        }),
        metric: Some(BudgetMetricV1::TerminalTokens),
    };
    let e = BridgeError::BudgetRefused {
        refusal: Box::new(refusal),
    };
    assert_eq!(e.client_message(), "budget refused");
    assert_eq!(e.to_string(), "budget refused");
    assert!(!e.is_transient());
    assert!(!e.is_resumable());
    let cause = bridge_core::execution_policy::NodeCauseV1::from_bridge_error(&e);
    assert_eq!(cause.failure_class, DiagnosticFailureClass::Config);
    assert_eq!(cause.code.as_str(), "bridge.budget_refused");
}
