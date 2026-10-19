//! Pure, phase-one provider fallback policy.
//!
//! This module deliberately decides only whether a frozen, already validated
//! candidate may be selected. Dispatch, retry and resume integration belong to
//! the next lane.

use crate::diagnostics::DiagnosticFailureClass;
use crate::ids::AgentId;
use std::collections::BTreeSet;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FallbackCauseV1 {
    QuotaExhausted,
    RegionRestricted,
    BillingFailure,
    RateLimited,
}

impl TryFrom<DiagnosticFailureClass> for FallbackCauseV1 {
    type Error = ();

    fn try_from(value: DiagnosticFailureClass) -> Result<Self, Self::Error> {
        match value {
            DiagnosticFailureClass::QuotaExhausted => Ok(Self::QuotaExhausted),
            DiagnosticFailureClass::RegionRestricted => Ok(Self::RegionRestricted),
            DiagnosticFailureClass::BillingFailure => Ok(Self::BillingFailure),
            DiagnosticFailureClass::RateLimited => Ok(Self::RateLimited),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingModeV1 {
    Metered,
    Subscription,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BillingIdentityV1 {
    pub agent: AgentId,
    pub effective_model: String,
    pub account_group: String,
    pub billing_mode: BillingModeV1,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResolvedBillingIdentityV1 {
    Mapped(BillingIdentityV1),
    Unmapped {
        agent: AgentId,
        effective_model: Option<String>,
    },
}

pub fn resolve_billing_identity(
    identities: &[BillingIdentityV1],
    agent: &AgentId,
    effective_model: Option<&str>,
) -> ResolvedBillingIdentityV1 {
    let mapped = effective_model.and_then(|model| {
        identities
            .iter()
            .find(|identity| identity.agent == *agent && identity.effective_model == model)
            .cloned()
    });
    mapped.map_or_else(
        || ResolvedBillingIdentityV1::Unmapped {
            agent: agent.clone(),
            effective_model: effective_model.map(ToOwned::to_owned),
        },
        ResolvedBillingIdentityV1::Mapped,
    )
}

/// Normalized capability posture, constructed during configuration admission.
/// Every field must compare equal; unknown equivalence is never a downgrade grant.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CandidatePostureV1 {
    pub kind: String,
    pub sandbox: Option<SandboxPostureV1>,
    pub cwd: String,
    pub session_cwd: String,
    pub cmd: Option<String>,
    pub args: Vec<String>,
    pub mode: Option<String>,
    pub mcp_delivery: String,
    pub mcp: BTreeSet<McpPostureV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SandboxPostureV1 {
    pub runtime: String,
    pub image: String,
    pub mount: String,
    pub access: String,
    pub egress: String,
    pub network: Option<String>,
    pub proxy: Option<String>,
    pub no_proxy: Option<String>,
    pub volumes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct McpPostureV1 {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    // name, literal value, environment reference; these are distinct authorities.
    pub env: BTreeSet<(String, Option<String>, Option<String>)>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FallbackCandidateV1 {
    pub agent: AgentId,
    pub model: String,
    pub posture: CandidatePostureV1,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FrozenFallbackChainV1 {
    pub id: String,
    pub source: FallbackCandidateV1,
    pub causes: BTreeSet<FallbackCauseV1>,
    pub steps: Vec<FallbackCandidateV1>,
}

/// Position in the original request's frozen chain, never a new source lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallbackPositionV1 {
    Source,
    Step(usize),
}

/// Select exactly once, using the original request and its first failure.
/// Later failures must retain this chain and advance its position.
pub fn select_fallback_chain<'a>(
    chains: &'a [FrozenFallbackChainV1],
    agent: &AgentId,
    model: &str,
    cause: DiagnosticFailureClass,
) -> Option<&'a FrozenFallbackChainV1> {
    let cause = FallbackCauseV1::try_from(cause).ok()?;
    let mut matches = chains.iter().filter(|chain| {
        chain.source.agent == *agent && chain.source.model == model && chain.causes.contains(&cause)
    });
    let chain = matches.next()?;
    // Fail closed if admission was bypassed and policies overlap.
    matches.next().is_none().then_some(chain)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExclusionEvidenceV1 {
    #[default]
    Unknown,
    Excluded,
    Present,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayEvidenceV1 {
    pub prompt_acceptance: ExclusionEvidenceV1,
    pub effects: ExclusionEvidenceV1,
    pub cleanup_completed: bool,
}

impl ReplayEvidenceV1 {
    fn permits_replay(self) -> bool {
        self.prompt_acceptance == ExclusionEvidenceV1::Excluded
            && self.effects == ExclusionEvidenceV1::Excluded
            && self.cleanup_completed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoutingDecisionV1 {
    OriginalFailure,
    ReplayUnsafe,
    Next(Box<FallbackCandidateV1>),
    Exhausted,
    PolicyViolation,
}

/// Decide from a frozen chain only. A malformed frozen policy fails closed;
/// validation should make that branch unreachable in production.
pub fn decide_fallback(
    chain: Option<&FrozenFallbackChainV1>,
    cause: DiagnosticFailureClass,
    evidence: ReplayEvidenceV1,
    position: FallbackPositionV1,
) -> RoutingDecisionV1 {
    let Ok(cause) = FallbackCauseV1::try_from(cause) else {
        return RoutingDecisionV1::OriginalFailure;
    };
    let Some(chain) = chain else {
        return RoutingDecisionV1::OriginalFailure;
    };
    let mut seen = BTreeSet::from([(chain.source.agent.as_str(), chain.source.model.as_str())]);
    if chain.steps.iter().any(|step| {
        !seen.insert((step.agent.as_str(), step.model.as_str()))
            || step.posture != chain.source.posture
    }) {
        return RoutingDecisionV1::PolicyViolation;
    }
    let next = match position {
        FallbackPositionV1::Source => 0,
        FallbackPositionV1::Step(index) if index < chain.steps.len() => index + 1,
        FallbackPositionV1::Step(_) => return RoutingDecisionV1::PolicyViolation,
    };
    let Some(step) = chain.steps.get(next) else {
        return RoutingDecisionV1::Exhausted;
    };
    if !chain.causes.contains(&cause) {
        return RoutingDecisionV1::OriginalFailure;
    }
    if !evidence.permits_replay() {
        return RoutingDecisionV1::ReplayUnsafe;
    }
    RoutingDecisionV1::Next(Box::new(step.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(agent: &str, model: &str, unrestricted: bool) -> FallbackCandidateV1 {
        FallbackCandidateV1 {
            agent: AgentId::parse(agent).unwrap(),
            model: model.into(),
            posture: CandidatePostureV1 {
                kind: "acp".into(),
                args: if unrestricted {
                    vec![]
                } else {
                    vec!["--read-only".into()]
                },
                ..Default::default()
            },
        }
    }

    fn chain() -> FrozenFallbackChainV1 {
        FrozenFallbackChainV1 {
            id: "quota".into(),
            source: candidate("source", "one", false),
            causes: BTreeSet::from([FallbackCauseV1::QuotaExhausted]),
            steps: vec![candidate("target", "two", false)],
        }
    }

    fn safe_replay_evidence() -> ReplayEvidenceV1 {
        ReplayEvidenceV1 {
            prompt_acceptance: ExclusionEvidenceV1::Excluded,
            effects: ExclusionEvidenceV1::Excluded,
            cleanup_completed: true,
        }
    }

    #[test]
    fn only_matching_safe_failures_advance_to_declared_target() {
        assert!(matches!(
            decide_fallback(
                Some(&chain()),
                DiagnosticFailureClass::QuotaExhausted,
                safe_replay_evidence(),
                FallbackPositionV1::Source,
            ),
            RoutingDecisionV1::Next(candidate) if candidate.agent.as_str() == "target"
        ));
        assert_eq!(
            decide_fallback(
                Some(&chain()),
                DiagnosticFailureClass::BillingFailure,
                safe_replay_evidence(),
                FallbackPositionV1::Source,
            ),
            RoutingDecisionV1::OriginalFailure
        );
    }

    #[test]
    fn prompt_or_cleanup_uncertainty_stops_replay_and_chain_is_bounded() {
        for evidence in [
            ReplayEvidenceV1 {
                prompt_acceptance: ExclusionEvidenceV1::Present,
                ..safe_replay_evidence()
            },
            ReplayEvidenceV1 {
                effects: ExclusionEvidenceV1::Present,
                ..safe_replay_evidence()
            },
            ReplayEvidenceV1::default(),
        ] {
            assert_eq!(
                decide_fallback(
                    Some(&chain()),
                    DiagnosticFailureClass::QuotaExhausted,
                    evidence,
                    FallbackPositionV1::Source
                ),
                RoutingDecisionV1::ReplayUnsafe
            );
        }
        assert_eq!(
            decide_fallback(
                Some(&chain()),
                DiagnosticFailureClass::QuotaExhausted,
                safe_replay_evidence(),
                FallbackPositionV1::Step(0),
            ),
            RoutingDecisionV1::Exhausted
        );
    }

    #[test]
    fn broader_target_posture_fails_closed() {
        let mut chain = chain();
        chain.steps[0].posture.args.clear();
        assert_eq!(
            decide_fallback(
                Some(&chain),
                DiagnosticFailureClass::QuotaExhausted,
                safe_replay_evidence(),
                FallbackPositionV1::Source,
            ),
            RoutingDecisionV1::PolicyViolation
        );
    }

    #[test]
    fn billing_identity_is_exact_and_unmapped_is_explicit() {
        let agent = AgentId::parse("source").unwrap();
        let identities = vec![BillingIdentityV1 {
            agent: agent.clone(),
            effective_model: "one".into(),
            account_group: "team-a".into(),
            billing_mode: BillingModeV1::Metered,
        }];
        assert!(matches!(
            resolve_billing_identity(&identities, &agent, Some("one")),
            ResolvedBillingIdentityV1::Mapped(_)
        ));
        assert!(matches!(
            resolve_billing_identity(&identities, &agent, Some("override")),
            ResolvedBillingIdentityV1::Unmapped { .. }
        ));
    }
}
