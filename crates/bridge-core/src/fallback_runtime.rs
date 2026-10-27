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
