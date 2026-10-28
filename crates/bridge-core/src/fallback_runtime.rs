//! Lazy, single-use provider fallback transitions. No dispatch surface calls this yet.
//! Owners supply operator policy and effective selections, never request metadata.
//! Accounting and billing remain owned by each selected attempt's existing preparation path.
use crate::error::BridgeError;
use crate::fs_custody::PinnedDirectoryV1;
use crate::routing::*;
use crate::translator::AccountingFailureV1;
use std::path::Path;

/// Retained source scope, not a lexical cwd equality assertion. Unprovable paths
/// (including opaque extra volume syntax) refuse fallback without affecting dispatch.
struct ScopeV1(Vec<(String, PinnedDirectoryV1)>);
impl ScopeV1 {
    fn freeze(posture: &CandidatePostureV1) -> Option<Self> {
        let mut paths = vec![posture.cwd.clone(), posture.session_cwd.clone()];
        if let Some(sandbox) = &posture.sandbox {
            if !sandbox.volumes.is_empty() {
                return None;
            }
            paths.push(sandbox.mount.clone());
        }
        let mut pins = Vec::new();
        for path in paths {
            let pin = PinnedDirectoryV1::open(Path::new(&path), "fallback scope").ok()?;
            let identity = pin.identity();
            if identity.dev.is_none() || identity.ino.is_none() || identity.btime.is_none() {
                return None;
            }
            pins.push((path, pin));
        }
        Some(Self(pins))
    }
    fn unchanged(&self) -> bool {
        self.0.iter().all(|(path, pin)| {
            PinnedDirectoryV1::open(Path::new(path), "fallback scope")
                .is_ok_and(|now| pin.identity() == now.identity())
        })
    }
}

/// Frozen before source dispatch. Not Clone: each failure consumes the current
/// position, preventing a second successor or an ordinary-retry escape.
pub struct FrozenFallbackInvocationV1 {
    chains: Vec<FrozenFallbackChainV1>,
    source: FallbackCandidateV1,
    selected: Option<usize>,
    position: FallbackPositionV1,
    scope: std::sync::Arc<ScopeV1>,
}
impl FrozenFallbackInvocationV1 {
    /// Only the effective source's local scope is pinned. No target resolution,
    /// model probe, accounting start, checkout or resource setup occurs here.
    /// None disables routing; it must never fail the source invocation.
    pub fn freeze(chains: Vec<FrozenFallbackChainV1>, source: FallbackCandidateV1) -> Option<Self> {
        let scope = ScopeV1::freeze(&source.posture)?;
        Some(Self {
            chains,
            source,
            selected: None,
            position: FallbackPositionV1::Source,
            scope: std::sync::Arc::new(scope),
        })
    }

    pub fn source(&self) -> &FallbackCandidateV1 {
        &self.source
    }

    /// Retain the same pinned objects through preparation and the first provider poll.
    pub fn scope_guard(&self) -> std::sync::Arc<dyn Fn() -> bool + Send + Sync> {
        let scope = self.scope.clone();
        std::sync::Arc::new(move || scope.unchanged())
    }

    /// Called only for a failed attempt after checked cleanup. Admission binds
    /// only the selected candidate's effective authority, before target execution.
    /// Any refusal preserves the complete original failure, including persistence.
    /// On success the owner starts a fresh accounting attempt with its own billing
    /// identity; unvisited candidates have no accounting or preparation effects.
    pub fn transition(
        mut self,
        failure: AccountingFailureV1,
        canceled: bool,
        admit: impl FnOnce(&FallbackCandidateV1) -> Result<FallbackCandidateV1, BridgeError>,
    ) -> Result<(Self, FallbackCandidateV1), AccountingFailureV1> {
        if canceled || failure.persistence.is_some() {
            return Err(failure);
        }
        let BridgeError::AgentFailure { diagnostic } = &failure.original else {
            return Err(failure);
        };
        if diagnostic.prompt_may_have_been_accepted() {
            return Err(failure);
        }
        if self.selected.is_none() {
            self.selected = select_fallback_chain(
                &self.chains,
                &self.source.agent,
                &self.source.model,
                diagnostic.class(),
            )
            .and_then(|chain| {
                self.chains
                    .iter()
                    .position(|entry| std::ptr::eq(entry, chain))
            });
        }
        let chain = self.selected.map(|index| &self.chains[index]);
        if chain.is_some_and(|chain| chain.source != self.source) {
            return Err(failure);
        }
        let RoutingDecisionV1::Next(candidate) =
            decide_fallback(chain, diagnostic.class(), failure.replay, self.position)
        else {
            return Err(failure);
        };
        if failure.cleanup != crate::ports::BackendCleanupDispositionV1::Complete
            || !self.scope.unchanged()
        {
            return Err(failure);
        }
        // Exact effective authority and selector, including invocation overrides.
        if !matches!(admit(&candidate), Ok(effective) if effective == *candidate)
            || !self.scope.unchanged()
        {
            return Err(failure);
        }
        self.position = match self.position {
            FallbackPositionV1::Source => FallbackPositionV1::Step(0),
            FallbackPositionV1::Step(index) => FallbackPositionV1::Step(index + 1),
        };
        Ok((self, *candidate))
    }
}

use crate::task_store::{CandidatePhaseV1, CandidateProgressV1, CandidateRejectionV1};

pub fn validate_fallback_authority_v1(
    chains: &[FrozenFallbackChainV1],
    source: &FallbackCandidateV1,
) -> Result<(), BridgeError> {
    let mut ids = std::collections::BTreeSet::new();
    let mut causes = std::collections::BTreeSet::new();
    if chains.is_empty()
        || chains.iter().any(|chain| {
            chain.id.is_empty()
                || !ids.insert(&chain.id)
                || chain.source != *source
                || chain.causes.is_empty()
                || chain.steps.is_empty()
                || chain.causes.iter().any(|cause| !causes.insert(*cause))
                || matches!(
                    decide_fallback(
                        Some(chain),
                        crate::diagnostics::DiagnosticFailureClass::QuotaExhausted,
                        ReplayEvidenceV1::default(),
                        FallbackPositionV1::Source
                    ),
                    RoutingDecisionV1::PolicyViolation
                )
        })
    {
        return Err(BridgeError::InvalidStateTransition);
    }
    Ok(())
}
impl CandidateRejectionV1 {
    fn replay(&self) -> ReplayEvidenceV1 {
        ReplayEvidenceV1 {
            prompt_acceptance: if self.non_acceptance
                && !self.diagnostic.prompt_may_have_been_accepted()
            {
                ExclusionEvidenceV1::Excluded
            } else {
                ExclusionEvidenceV1::Unknown
            },
            effects: if self.no_effects {
                ExclusionEvidenceV1::Excluded
            } else {
                ExclusionEvidenceV1::Unknown
            },
            cleanup_completed: self.cleanup_complete
                && self.persistence_error.is_none()
                && !self.canceled,
        }
    }
}
impl CandidateProgressV1 {
    /// Structural validation never grants authority: restore also revalidates the
    /// operator's current frozen policy, effective source and pinned scope.
    pub fn validate(&self) -> Result<(), BridgeError> {
        validate_fallback_authority_v1(&self.chains, &self.source)?;
        let invalid = || BridgeError::InvalidStateTransition;
        if self.schema_version != 1
            || self.scope.is_empty()
            || self.selected_chain.is_some_and(|i| i >= self.chains.len())
        {
            return Err(invalid());
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut contexts = std::collections::BTreeSet::new();
        let mut retries = 0u32;
        for (i, visit) in self.visits.iter().enumerate() {
            if visit.accounting_id.is_empty()
                || !ids.insert(&visit.accounting_id)
                || !contexts.insert(visit.cold_context.as_str())
                || (i == 0 && visit.position != 0)
            {
                return Err(invalid());
            }
            if i == 0 {
                continue;
            }
            let prior = &self.visits[i - 1];
            if visit.owner.execution_id != prior.owner.execution_id {
                return Err(invalid());
            }
            let rejection = prior.rejection.as_ref().ok_or_else(invalid)?;
            if visit.position == prior.position {
                retries = retries.checked_add(1).ok_or_else(invalid)?;
                if !rejection.cleanup_complete
                    || rejection.canceled
                    || rejection.persistence_error.is_some()
                {
                    return Err(invalid());
                }
            } else {
                let chain = self.selected_chain.map(|index| &self.chains[index]);
                let position = if prior.position == 0 {
                    FallbackPositionV1::Source
                } else {
                    FallbackPositionV1::Step(prior.position - 1)
                };
                if visit.position != prior.position + 1
                    || !prior.phase.dispatched()
                    || !matches!(
                        decide_fallback(
                            chain,
                            rejection.diagnostic.class(),
                            rejection.replay(),
                            position
                        ),
                        RoutingDecisionV1::Next(_)
                    )
                {
                    return Err(invalid());
                }
            }
        }
        let terminal_valid = self.terminal.as_ref().is_none_or(|output| {
            self.visits.last().is_some_and(|visit| {
                if output.ok {
                    visit.phase.dispatched() && visit.rejection.is_none()
                } else {
                    visit.phase != CandidatePhaseV1::PollFenced
                        && visit.rejection.as_ref().is_some_and(|r| r.cleanup_complete)
                }
            })
        });
        if retries != self.retries_consumed || !terminal_valid {
            return Err(invalid());
        }
        Ok(())
    }

    /// Preserve the last polled provider's failure when a successor fails before
    /// polling. A polled successor with no rejection cannot borrow an older failure.
    pub fn primary_failure(&self) -> Option<&CandidateRejectionV1> {
        self.visits
            .iter()
            .rev()
            .find(|visit| visit.phase.dispatched())
            .and_then(|visit| visit.rejection.as_ref())
    }

    /// Legal publication preserves prior owners/evidence and cannot unprepare a
    /// candidate. The store additionally compares the expected bytes under a transaction.
    pub fn follows(&self, old: &Self) -> Result<(), BridgeError> {
        self.validate()?;
        let mut previous = old.clone();
        previous.selected_chain = self.selected_chain;
        previous.retries_consumed = self.retries_consumed;
        previous.terminal = self.terminal.clone();
        if old.terminal.is_some()
            || old.selected_chain.is_some() && old.selected_chain != self.selected_chain
            || self.visits.len() < old.visits.len()
            || self.visits.len() > old.visits.len() + 1
        {
            return Err(BridgeError::InvalidStateTransition);
        }
        if let Some(last) = previous.visits.last_mut() {
            let next = &self.visits[old.visits.len() - 1];
            if (next.phase as u8) < (last.phase as u8)
                || (next.phase as u8) > (last.phase as u8) + 1
                || last.rejection.is_some() && last.rejection != next.rejection
            {
                return Err(BridgeError::InvalidStateTransition);
            }
            last.phase = next.phase;
            last.rejection = next.rejection.clone();
        }
        if self.visits.len() > old.visits.len() {
            let next = self.visits.last().unwrap();
            if next.phase != CandidatePhaseV1::Preparing || next.rejection.is_some() {
                return Err(BridgeError::InvalidStateTransition);
            }
            previous.visits.push(next.clone());
        }
        if previous != *self {
            return Err(BridgeError::InvalidStateTransition);
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub enum CandidateRecoveryV1 {
    Continue {
        candidate: Box<FallbackCandidateV1>,
        preceding_failure: Box<CandidateRejectionV1>,
    },
    Completed(crate::task_store::CandidateOutputV1),
}
impl FrozenFallbackInvocationV1 {
    pub fn progress(&self) -> CandidateProgressV1 {
        CandidateProgressV1 {
            schema_version: 1,
            chains: self.chains.clone(),
            source: self.source.clone(),
            scope: self
                .scope
                .0
                .iter()
                .map(|(_, pin)| pin.identity().clone())
                .collect(),
            selected_chain: self.selected,
            retries_consumed: 0,
            visits: vec![],
            terminal: None,
        }
    }

    /// Any error interrupts recovery. This is a read-only decision: the caller
    /// must CAS-publish a Preparing visit before allowing preparation to escape.
    /// A second recovery of that visit refuses rather than repeating preparation.
    pub fn restore(
        mut self,
        progress: &CandidateProgressV1,
        admit: impl FnOnce(&FallbackCandidateV1) -> Result<FallbackCandidateV1, BridgeError>,
    ) -> Result<(Self, CandidateRecoveryV1), BridgeError> {
        progress.validate()?;
        let frozen = self.progress();
        if frozen.chains != progress.chains
            || frozen.source != progress.source
            || frozen.scope != progress.scope
            || !self.scope.unchanged()
        {
            return Err(BridgeError::InvalidStateTransition);
        }
        if let Some(output) = &progress.terminal {
            return Ok((self, CandidateRecoveryV1::Completed(output.clone())));
        }
        let last = progress
            .visits
            .last()
            .ok_or(BridgeError::InvalidStateTransition)?;
        let rejection = last
            .rejection
            .as_ref()
            .ok_or(BridgeError::InvalidStateTransition)?;
        if !last.phase.dispatched() {
            return Err(BridgeError::InvalidStateTransition);
        }
        self.selected = progress.selected_chain.or_else(|| {
            select_fallback_chain(
                &self.chains,
                &self.source.agent,
                &self.source.model,
                rejection.diagnostic.class(),
            )
            .and_then(|chain| self.chains.iter().position(|entry| entry == chain))
        });
        self.position = if last.position == 0 {
            FallbackPositionV1::Source
        } else {
            FallbackPositionV1::Step(last.position - 1)
        };
        let RoutingDecisionV1::Next(candidate) = decide_fallback(
            self.selected.map(|i| &self.chains[i]),
            rejection.diagnostic.class(),
            rejection.replay(),
            self.position,
        ) else {
            return Err(BridgeError::InvalidStateTransition);
        };
        if admit(&candidate)? != *candidate || !self.scope.unchanged() {
            return Err(BridgeError::InvalidStateTransition);
        }
        self.position = if last.position == 0 {
            FallbackPositionV1::Step(0)
        } else {
            FallbackPositionV1::Step(last.position)
        };
        Ok((
            self,
            CandidateRecoveryV1::Continue {
                candidate,
                preceding_failure: Box::new(rejection.clone()),
            },
        ))
    }
}
