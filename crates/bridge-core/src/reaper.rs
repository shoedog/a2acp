//! Shared container-reaping primitives (used by the :rw ContainerRwBackend and the :ro AcpBackend path).
//! Detached + idempotent so a `Drop` (which may fire off-runtime at process shutdown) never blocks/panics.
use crate::attempt_activity::{MonotonicClock, SystemMonotonicClock};
use crate::execution_policy::Sha256HexV1;
use crate::ids::{AttemptId, NodeId};
use crate::resource_flight::BoundedRecoveryReasonV1;
use crate::resource_flight::{
    ResourceActionDispositionV1, ResourceActionResultV1, ResourceFlightIdV1, ResourceIdentityV1,
};
use crate::retained_resource_flight::CleanupDeadlineTransferV1;
use crate::retained_resource_flight::{
    ContainerRemovalObservationV1, InMemoryResourceFlightJournal,
    NoopResourceFlightResultPublisher, ResourceActionIntentV1, ResourceFlightJournal,
    ResourceFlightKeyV1, ResourceFlightOwnerV1, ResourceFlightRegistryV1,
    ResourceFlightReservationV1, ResourceFlightResultPublisher, RetainedResourceFlight,
    RetainedResourceFlightConfigV1,
};
use crate::run_identity::CanonicalContainerOwnershipV1;
use futures::FutureExt;
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;

pub const CONTAINER_IDENTITY_FORMAT: &str = "{{.Id}}{{\"\\t\"}}{{json .Config.Labels}}";
pub const CONTAINER_INVENTORY_FORMAT: &str = "{{.ID}}{{\"\\t\"}}{{.Names}}";

const CONTAINER_START_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const CONTAINER_START_STATUS_MAX_BYTES: u64 = 64;
const CONTAINER_IDENTITY_MAX_BYTES: u64 = 64 * 1024;
/// How long after the exact-ID removal command exits the container may still be observed as
/// present before the removal is reported failed. It bounds every inventory child too, and is
/// always further bounded by the removal's original deadline.
const CONTAINER_REMOVAL_OBSERVATION_WINDOW: Duration = Duration::from_secs(2);
const CONTAINER_REMOVAL_OBSERVATION_INTERVAL: Duration = Duration::from_millis(50);

/// `(runtime, name) -> fire-and-forget reap`. Injectable so tests don't spawn Docker.
pub type ReapFn = Arc<dyn Fn(String, String) + Send + Sync>;

/// One bounded removal attempt. The result is metadata-only and safe to share
/// with operation-owned teardown diagnostics.
pub type ReapAttemptFn = Arc<
    dyn Fn(String, String) -> Pin<Box<dyn Future<Output = Result<(), ReapFailure>> + Send>>
        + Send
        + Sync,
>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerRuntimeIdentityV1 {
    pub immutable_container_id: String,
    pub ownership_labels: Vec<(String, String)>,
}

pub type ContainerIdentityProbeFn = Arc<
    dyn Fn(
            String,
            String,
        )
            -> Pin<Box<dyn Future<Output = Result<ContainerRuntimeIdentityV1, ReapFailure>> + Send>>
        + Send
        + Sync,
>;

pub type ContainerSubordinateCleanupFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send>> + Send + Sync>;

/// Attempt-owned durable inputs for one protected container generation.
pub struct DurableContainerFlightV3 {
    pub(crate) attempt_id: AttemptId,
    pub(crate) generation: String,
    pub(crate) flight_id: ResourceFlightIdV1,
    pub(crate) registry: Arc<ResourceFlightRegistryV1>,
    pub(crate) journal: Arc<dyn ResourceFlightJournal>,
    pub(crate) owner: ResourceFlightOwnerV1,
    pub(crate) result_publisher: Arc<dyn ResourceFlightResultPublisher>,
}

impl std::fmt::Debug for DurableContainerFlightV3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableContainerFlightV3")
            .field("attempt_id", &self.attempt_id)
            .field("generation", &self.generation)
            .field("flight_id", &self.flight_id)
            .finish_non_exhaustive()
    }
}

enum ContainerFlightProvisionV1 {
    LegacyV2,
    DurableV3(DurableContainerFlightV3),
}

/// Runtime-observed state of one exact named container. `NotStarted` is deliberately narrower than
/// generic runtime unavailability: it is returned only when the runtime itself says the object remains
/// in a pre-start state. Callers must preserve their existing diagnosis for `Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerStartState {
    NotStarted,
    Started,
    Unknown,
}

/// One bounded exact-name state observation. Injectable so ACP lifecycle tests never invoke Docker.
pub type ContainerStartProbeFn = Arc<
    dyn Fn(String, String) -> Pin<Box<dyn Future<Output = ContainerStartState> + Send>>
        + Send
        + Sync,
>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReapFailure {
    Spawn,
    Timeout,
    NonZeroExit,
    WorkerPanicked,
    IdentityUnavailable,
    IdentityChanged,
    OwnershipLabelsChanged,
    FlightRefused,
    SubordinateCleanup,
    AlreadyGone,
}

impl ReapFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Spawn => "container.reap.spawn_failed",
            Self::Timeout => "container.reap.timeout",
            Self::NonZeroExit => "container.reap.nonzero_exit",
            Self::WorkerPanicked => "container.reap.worker_panicked",
            Self::IdentityUnavailable => "container.reap.identity_unavailable",
            Self::IdentityChanged => "container.reap.identity_changed",
            Self::OwnershipLabelsChanged => "container.reap.ownership_labels_changed",
            Self::FlightRefused => "container.reap.flight_refused",
            Self::SubordinateCleanup => "container.reap.subordinate_cleanup_failed",
            Self::AlreadyGone => "container.reap.already_gone",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReapState {
    NotStarted,
    Running,
    Settled(Result<(), ReapFailure>),
}

struct ReapShared {
    state: StdMutex<ReapState>,
    settled: tokio::sync::Notify,
}

#[derive(Clone)]
struct ManagedReapFlightV1 {
    identity: ResourceIdentityV1,
    selector: String,
    ownership: CanonicalContainerOwnershipV1,
    identity_probe: ContainerIdentityProbeFn,
    flight: Arc<RetainedResourceFlight>,
    owner: ResourceFlightOwnerV1,
    protected_v3: bool,
    subordinate: Arc<StdMutex<Option<ContainerSubordinateCleanupFn>>>,
    clock: Arc<dyn MonotonicClock>,
}

#[derive(Clone)]
struct ProductionIdentityAuthorityV1 {
    selector: String,
    identity_probe: ContainerIdentityProbeFn,
    captured: Arc<StdMutex<Option<ContainerRuntimeIdentityV1>>>,
}

/// Shared, cancellation-safe, joinable ownership for one named container reap.
/// The worker never owns an operation observer; observed callers only await and
/// locally report the shared metadata-only result.
#[derive(Clone)]
pub struct ReapController {
    runtime: String,
    name: String,
    attempt: ReapAttemptFn,
    start_probe: Option<ContainerStartProbeFn>,
    managed: Option<ManagedReapFlightV1>,
    production_identity: Option<ProductionIdentityAuthorityV1>,
    shared: Arc<ReapShared>,
}

impl std::fmt::Debug for ReapController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReapController")
            .field("runtime", &self.runtime)
            .field("name", &self.name)
            .field("result", &self.result())
            .finish_non_exhaustive()
    }
}

impl ReapController {
    pub fn new(
        runtime: impl Into<String>,
        name: impl Into<String>,
        attempt: ReapAttemptFn,
    ) -> Self {
        Self {
            runtime: runtime.into(),
            name: name.into(),
            attempt,
            start_probe: None,
            managed: None,
            production_identity: None,
            shared: Arc::new(ReapShared {
                state: StdMutex::new(ReapState::NotStarted),
                settled: tokio::sync::Notify::new(),
            }),
        }
    }

    /// Source-compatible adapter for existing injectable fire-and-forget tests
    /// and constructors. Invocation completion is the only result that legacy
    /// closures can expose, so it settles successfully after the call returns.
    pub fn from_legacy(
        runtime: impl Into<String>,
        name: impl Into<String>,
        reap_fn: ReapFn,
    ) -> Self {
        let attempt: ReapAttemptFn = Arc::new(move |runtime, name| {
            let reap_fn = Arc::clone(&reap_fn);
            Box::pin(async move {
                reap_fn(runtime, name);
                Ok(())
            })
        });
        Self::new(runtime, name, attempt)
    }

    pub fn production(runtime: impl Into<String>, name: impl Into<String>) -> Self {
        Self::production_with_timeout(runtime, name, Duration::from_secs(10))
    }

    fn production_with_timeout(
        runtime: impl Into<String>,
        name: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        let selector = name.into();
        let identity_probe = production_identity_probe(timeout);
        let captured = Arc::new(StdMutex::new(None));
        let authority = ProductionIdentityAuthorityV1 {
            selector: selector.clone(),
            identity_probe: Arc::clone(&identity_probe),
            captured: Arc::clone(&captured),
        };
        let attempt: ReapAttemptFn = Arc::new(move |runtime, selector| {
            let timeout = timeout;
            let identity_probe = Arc::clone(&identity_probe);
            let captured = Arc::clone(&captured);
            Box::pin(async move {
                let expected = captured
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let Some(expected) = expected else {
                    return match identity_probe(runtime, selector).await {
                        Err(ReapFailure::AlreadyGone) => Ok(()),
                        Ok(_) => Err(ReapFailure::IdentityUnavailable),
                        Err(error) => Err(error),
                    };
                };
                let observed = match identity_probe(runtime.clone(), selector).await {
                    Err(ReapFailure::AlreadyGone) => return Ok(()),
                    result => result?,
                };
                if observed.immutable_container_id != expected.immutable_container_id {
                    return Err(ReapFailure::IdentityChanged);
                }
                if observed.ownership_labels != expected.ownership_labels {
                    return Err(ReapFailure::OwnershipLabelsChanged);
                }
                remove_container_id(&runtime, &expected.immutable_container_id, timeout).await
            })
        });
        let mut controller = Self::new(runtime, selector, attempt)
            .with_start_probe(production_start_probe(CONTAINER_START_PROBE_TIMEOUT));
        controller.production_identity = Some(authority);
        controller
    }

    pub fn managed_production_v2(
        identity: ResourceIdentityV1,
        selector: impl Into<String>,
        ownership: CanonicalContainerOwnershipV1,
        subordinate: ContainerSubordinateCleanupFn,
    ) -> Result<Self, ReapFailure> {
        Self::managed(
            identity,
            selector,
            ownership,
            production_remove_attempt(Duration::from_secs(10)),
            production_identity_probe(Duration::from_secs(2)),
            subordinate,
            ContainerFlightProvisionV1::LegacyV2,
        )
    }

    pub fn managed_production_v3(
        identity: ResourceIdentityV1,
        selector: impl Into<String>,
        ownership: CanonicalContainerOwnershipV1,
        subordinate: ContainerSubordinateCleanupFn,
        durable: DurableContainerFlightV3,
    ) -> Result<Self, ReapFailure> {
        Self::managed(
            identity,
            selector,
            ownership,
            production_remove_attempt(Duration::from_secs(10)),
            production_identity_probe(Duration::from_secs(2)),
            subordinate,
            ContainerFlightProvisionV1::DurableV3(durable),
        )
    }

    pub fn managed_legacy_v2(
        identity: ResourceIdentityV1,
        selector: impl Into<String>,
        ownership: CanonicalContainerOwnershipV1,
        attempt: ReapAttemptFn,
        identity_probe: ContainerIdentityProbeFn,
        subordinate: ContainerSubordinateCleanupFn,
    ) -> Result<Self, ReapFailure> {
        Self::managed(
            identity,
            selector,
            ownership,
            attempt,
            identity_probe,
            subordinate,
            ContainerFlightProvisionV1::LegacyV2,
        )
    }

    pub fn managed_durable_v3(
        identity: ResourceIdentityV1,
        selector: impl Into<String>,
        ownership: CanonicalContainerOwnershipV1,
        attempt: ReapAttemptFn,
        identity_probe: ContainerIdentityProbeFn,
        subordinate: ContainerSubordinateCleanupFn,
        durable: DurableContainerFlightV3,
    ) -> Result<Self, ReapFailure> {
        Self::managed(
            identity,
            selector,
            ownership,
            attempt,
            identity_probe,
            subordinate,
            ContainerFlightProvisionV1::DurableV3(durable),
        )
    }

    fn managed(
        identity: ResourceIdentityV1,
        selector: impl Into<String>,
        ownership: CanonicalContainerOwnershipV1,
        attempt: ReapAttemptFn,
        identity_probe: ContainerIdentityProbeFn,
        subordinate: ContainerSubordinateCleanupFn,
        provision: ContainerFlightProvisionV1,
    ) -> Result<Self, ReapFailure> {
        let (generation, runtime, immutable_container_id, digest) = match &identity {
            ResourceIdentityV1::ManagedContainer {
                generation,
                runtime,
                immutable_container_id,
                ownership_labels_digest,
            } => (
                generation.clone(),
                runtime.clone(),
                immutable_container_id.clone(),
                ownership_labels_digest.clone(),
            ),
            _ => return Err(ReapFailure::IdentityUnavailable),
        };
        if immutable_container_id.is_empty() || digest != *ownership.digest() {
            return Err(ReapFailure::OwnershipLabelsChanged);
        }
        let (attempt_id, flight_id, registry, journal, owner, publisher, protected_v3) =
            match provision {
                ContainerFlightProvisionV1::LegacyV2 => {
                    let attempt_id = AttemptId::mint().map_err(|_| ReapFailure::FlightRefused)?;
                    let owner = ResourceFlightOwnerV1::new(
                        NodeId::parse("container-spawn").map_err(|_| ReapFailure::FlightRefused)?,
                        generation.clone(),
                    )
                    .map_err(|_| ReapFailure::FlightRefused)?;
                    (
                        attempt_id.clone(),
                        ResourceFlightIdV1::mint().map_err(|_| ReapFailure::FlightRefused)?,
                        Arc::new(ResourceFlightRegistryV1::new(attempt_id)),
                        Arc::new(InMemoryResourceFlightJournal::new(64))
                            as Arc<dyn ResourceFlightJournal>,
                        owner,
                        Arc::new(NoopResourceFlightResultPublisher)
                            as Arc<dyn ResourceFlightResultPublisher>,
                        false,
                    )
                }
                ContainerFlightProvisionV1::DurableV3(durable) => {
                    if durable.generation != generation {
                        return Err(ReapFailure::FlightRefused);
                    }
                    (
                        durable.attempt_id,
                        durable.flight_id,
                        durable.registry,
                        durable.journal,
                        durable.owner,
                        durable.result_publisher,
                        true,
                    )
                }
            };
        let clock: Arc<dyn MonotonicClock> = Arc::new(SystemMonotonicClock::start());
        let reservation = registry
            .reserve(RetainedResourceFlightConfigV1 {
                flight_id,
                attempt_id,
                key: ResourceFlightKeyV1::ContainerGeneration { generation },
                journal,
                clock: Arc::clone(&clock),
                result_publisher: publisher,
            })
            .map_err(|_| ReapFailure::FlightRefused)?;
        let ResourceFlightReservationV1::Created(flight) = reservation else {
            return Err(ReapFailure::FlightRefused);
        };
        if protected_v3 {
            flight.attach_owner(owner.clone())
        } else {
            flight.attach_owner_legacy_v2(owner.clone())
        }
        .map_err(|_| ReapFailure::FlightRefused)?;
        flight
            .journal_container_identity(identity.clone())
            .map_err(|_| ReapFailure::FlightRefused)?;
        Ok(Self {
            runtime,
            name: immutable_container_id,
            attempt,
            start_probe: None,
            production_identity: None,
            managed: Some(ManagedReapFlightV1 {
                identity,
                selector: selector.into(),
                ownership,
                identity_probe,
                flight,
                owner,
                protected_v3,
                subordinate: Arc::new(StdMutex::new(Some(subordinate))),
                clock,
            }),
            shared: Arc::new(ReapShared {
                state: StdMutex::new(ReapState::NotStarted),
                settled: tokio::sync::Notify::new(),
            }),
        })
    }

    #[must_use]
    pub fn is_protected_v3(&self) -> bool {
        self.managed
            .as_ref()
            .is_some_and(|managed| managed.protected_v3)
    }

    /// Attach the bridge-owned exact-name start observer. Legacy controllers intentionally omit this
    /// active runtime seam and retain their historical handshake behavior.
    #[doc(hidden)]
    pub fn with_start_probe(mut self, start_probe: ContainerStartProbeFn) -> Self {
        self.start_probe = Some(start_probe);
        self
    }

    #[doc(hidden)]
    pub fn has_start_probe(&self) -> bool {
        self.start_probe.is_some()
    }

    /// Observe the exact named container once. A panicking injected observer fails closed to `Unknown`;
    /// only positive runtime evidence can become `NotStarted` or `Started`.
    #[doc(hidden)]
    pub async fn probe_start_state(&self) -> ContainerStartState {
        let Some(probe) = &self.start_probe else {
            return ContainerStartState::Unknown;
        };
        let runtime = self.runtime.clone();
        let name = self.name.clone();
        let future =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| probe(runtime, name))) {
                Ok(future) => future,
                Err(_) => return ContainerStartState::Unknown,
            };
        match std::panic::AssertUnwindSafe(future).catch_unwind().await {
            Ok(state) => state,
            Err(_) => ContainerStartState::Unknown,
        }
    }

    /// Capture the immutable identity at the positive start/discovery boundary.
    /// Teardown may only revalidate this evidence; it never mints authority.
    pub async fn capture_production_identity(&self) -> Result<(), ReapFailure> {
        let Some(authority) = &self.production_identity else {
            return Ok(());
        };
        let observed =
            (authority.identity_probe)(self.runtime.clone(), authority.selector.clone()).await?;
        let mut captured = authority
            .captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match captured.as_ref() {
            None => {
                *captured = Some(observed);
                Ok(())
            }
            Some(expected)
                if expected.immutable_container_id == observed.immutable_container_id
                    && expected.ownership_labels == observed.ownership_labels =>
            {
                Ok(())
            }
            Some(expected)
                if expected.immutable_container_id != observed.immutable_container_id =>
            {
                Err(ReapFailure::IdentityChanged)
            }
            Some(_) => Err(ReapFailure::OwnershipLabelsChanged),
        }
    }

    pub fn transfer_cleanup_deadline(
        &self,
        reason: BoundedRecoveryReasonV1,
    ) -> Result<CleanupDeadlineTransferV1, ReapFailure> {
        let managed = self
            .managed
            .as_ref()
            .filter(|managed| managed.protected_v3)
            .ok_or(ReapFailure::FlightRefused)?;
        managed
            .flight
            .transfer_cleanup_deadline_now(reason)
            .map_err(|_| ReapFailure::FlightRefused)
    }

    pub fn attach_owner(&self, owner: ResourceFlightOwnerV1) -> Result<bool, ReapFailure> {
        let managed = self.managed.as_ref().ok_or(ReapFailure::FlightRefused)?;
        if managed.protected_v3 {
            managed.flight.attach_owner(owner)
        } else {
            managed.flight.attach_owner_legacy_v2(owner)
        }
        .map_err(|_| ReapFailure::FlightRefused)
    }

    async fn drive_managed(
        runtime: String,
        immutable_container_id: String,
        attempt: ReapAttemptFn,
        managed: ManagedReapFlightV1,
    ) -> Result<(), ReapFailure> {
        let subordinate = managed
            .subordinate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or(ReapFailure::FlightRefused)?;
        let subordinate_action = async move {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| subordinate())) {
                Ok(future) => match std::panic::AssertUnwindSafe(future).catch_unwind().await {
                    Ok(result) => result,
                    Err(_) => Err(()),
                },
                Err(_) => Err(()),
            }
        };

        let dispatch = managed
            .flight
            .close_admission()
            .and_then(|_| {
                managed.flight.journal_intent(ResourceActionIntentV1 {
                    initiator: managed.owner.clone(),
                    capability_digest: Sha256HexV1::digest(
                        &serde_json::to_vec(&managed.identity)
                            .unwrap_or_else(|_| b"managed-container-v1".to_vec()),
                    ),
                    cause: None,
                })
            })
            .and_then(|_| managed.flight.begin_journaled_dispatch().map(|_| ()));
        if dispatch.is_err() {
            let subordinate_result = subordinate_action.await;
            let local_result = if managed.protected_v3 && subordinate_result.is_err() {
                Err(ReapFailure::SubordinateCleanup)
            } else {
                Err(ReapFailure::FlightRefused)
            };
            let adopted = managed
                .flight
                .settle_failed_before_dispatch(ResourceActionResultV1 {
                    disposition: ResourceActionDispositionV1::Failed,
                    duration_ms: managed.clock.elapsed_ms(),
                    recovery_owner: None,
                    cause: None,
                })
                .map_err(|_| ReapFailure::FlightRefused)?;
            return project_managed_result(adopted, local_result);
        }

        let container_action = async {
            let probe = Arc::clone(&managed.identity_probe);
            let selector = managed.selector.clone();
            let observed = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                probe(runtime.clone(), selector)
            })) {
                Ok(future) => match std::panic::AssertUnwindSafe(future).catch_unwind().await {
                    Ok(result) => result,
                    Err(_) => Err(ReapFailure::IdentityUnavailable),
                },
                Err(_) => Err(ReapFailure::IdentityUnavailable),
            };
            match observed {
                Err(ReapFailure::AlreadyGone) => {
                    (Ok(()), false, Some(ReapFailure::AlreadyGone), Vec::new())
                }
                Ok(observed) if observed.immutable_container_id != immutable_container_id => (
                    Err(ReapFailure::IdentityChanged),
                    false,
                    Some(ReapFailure::IdentityChanged),
                    Vec::new(),
                ),
                Ok(observed)
                    if managed
                        .ownership
                        .validate_observed(&observed.ownership_labels)
                        .is_err() =>
                {
                    (
                        Err(ReapFailure::OwnershipLabelsChanged),
                        false,
                        Some(ReapFailure::OwnershipLabelsChanged),
                        Vec::new(),
                    )
                }
                Ok(observed) => {
                    let canonical_keys: std::collections::BTreeSet<_> = managed
                        .ownership
                        .ordered()
                        .iter()
                        .map(|(key, _)| key.as_str())
                        .collect();
                    let mut noncanonical_labels: Vec<_> = observed
                        .ownership_labels
                        .iter()
                        .filter(|(key, _)| !canonical_keys.contains(key.as_str()))
                        .cloned()
                        .collect();
                    noncanonical_labels.sort();
                    if !noncanonical_labels.is_empty() {
                        tracing::debug!(
                            ?noncanonical_labels,
                            "container identity included noncanonical a2a labels"
                        );
                    }
                    let removal_id = immutable_container_id.clone();
                    let future = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        attempt(runtime.clone(), removal_id)
                    }));
                    let removal_result = match future {
                        Ok(future) => {
                            match std::panic::AssertUnwindSafe(future).catch_unwind().await {
                                Ok(result) => result,
                                Err(_) => Err(ReapFailure::WorkerPanicked),
                            }
                        }
                        Err(_) => Err(ReapFailure::WorkerPanicked),
                    };
                    let failure = removal_result.as_ref().err().copied();
                    let removed = removal_result.is_ok();
                    (removal_result, removed, failure, noncanonical_labels)
                }
                Err(error) => (Err(error), false, Some(error), Vec::new()),
            }
        };
        let (
            subordinate_result,
            (container_result, removed, removal_failure, observed_noncanonical_a2a_labels),
        ) = tokio::join!(subordinate_action, container_action);
        let local_result =
            if managed.protected_v3 && subordinate_result.is_err() && container_result.is_ok() {
                Err(ReapFailure::SubordinateCleanup)
            } else {
                container_result
            };
        if managed
            .flight
            .journal_container_removal(ContainerRemovalObservationV1 {
                immutable_container_id,
                observed_noncanonical_a2a_labels,
                removed,
                failure_code: removal_failure.map(|failure| failure.code().to_owned()),
            })
            .is_err()
        {
            tracing::warn!("container removal observation could not be journaled after action");
        }
        let adopted = managed
            .flight
            .settle(ResourceActionResultV1 {
                disposition: if local_result.is_ok() {
                    ResourceActionDispositionV1::Complete
                } else {
                    ResourceActionDispositionV1::Failed
                },
                duration_ms: managed.clock.elapsed_ms(),
                recovery_owner: None,
                cause: None,
            })
            .map_err(|_| ReapFailure::FlightRefused)?;
        project_managed_result(adopted, local_result)
    }

    fn ensure_started(&self) {
        let should_start = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *state == ReapState::NotStarted {
                *state = ReapState::Running;
                true
            } else {
                false
            }
        };
        if !should_start {
            return;
        }

        let runtime = self.runtime.clone();
        let name = self.name.clone();
        let attempt = Arc::clone(&self.attempt);
        let managed = self.managed.clone();
        let shared = Arc::clone(&self.shared);
        spawn_detached(async move {
            let result = if let Some(managed) = managed {
                Self::drive_managed(runtime, name, attempt, managed).await
            } else {
                let attempt_future = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    attempt(runtime, name)
                }));
                match attempt_future {
                    Ok(future) => match std::panic::AssertUnwindSafe(future).catch_unwind().await {
                        Ok(result) => result,
                        Err(_) => Err(ReapFailure::WorkerPanicked),
                    },
                    Err(_) => Err(ReapFailure::WorkerPanicked),
                }
            };
            *shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = ReapState::Settled(result);
            shared.settled.notify_waiters();
        });
    }

    /// Start or join the single removal attempt and return its stable result.
    pub async fn reap_observed(&self) -> Result<(), ReapFailure> {
        self.ensure_started();
        loop {
            // Register before sampling state so settlement cannot be lost between
            // the sample and the await.
            let notified = self.shared.settled.notified();
            let state = *self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match state {
                ReapState::Settled(result) => return result,
                ReapState::NotStarted => self.ensure_started(),
                ReapState::Running => notified.await,
            }
        }
    }

    /// Start the same single attempt without retaining any operation observer.
    pub fn reap_detached(&self) {
        self.ensure_started();
    }

    pub fn result(&self) -> Option<Result<(), ReapFailure>> {
        match *self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            ReapState::Settled(result) => Some(result),
            ReapState::NotStarted | ReapState::Running => None,
        }
    }
}

fn project_managed_result(
    adopted: ResourceActionResultV1,
    local_result: Result<(), ReapFailure>,
) -> Result<(), ReapFailure> {
    match adopted.disposition {
        ResourceActionDispositionV1::Complete | ResourceActionDispositionV1::NotNeeded => Ok(()),
        ResourceActionDispositionV1::Failed => {
            Err(local_result.err().unwrap_or(ReapFailure::FlightRefused))
        }
        ResourceActionDispositionV1::Partial | ResourceActionDispositionV1::Unknown => {
            Err(local_result
                .err()
                .unwrap_or(ReapFailure::IdentityUnavailable))
        }
    }
}

/// Observe the immutable ID and complete `a2a.*` ownership label set for one
/// runtime selector. This is evidence only; it cannot remove a container.
pub async fn production_container_identity(
    runtime: &str,
    selector: &str,
) -> Result<ContainerRuntimeIdentityV1, ReapFailure> {
    observe_container_identity(runtime, selector, Duration::from_secs(2)).await
}

fn production_identity_probe(timeout: Duration) -> ContainerIdentityProbeFn {
    Arc::new(move |runtime, selector| {
        Box::pin(async move { observe_container_identity(&runtime, &selector, timeout).await })
    })
}

fn production_remove_attempt(timeout: Duration) -> ReapAttemptFn {
    Arc::new(move |runtime, immutable_id| {
        Box::pin(async move { remove_container_id(&runtime, &immutable_id, timeout).await })
    })
}

/// Parse the exact bytes emitted by [`CONTAINER_IDENTITY_FORMAT`].
pub fn parse_container_identity(bytes: &[u8]) -> Result<ContainerRuntimeIdentityV1, ReapFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| ReapFailure::IdentityUnavailable)?;
    let text = text.trim_end_matches(['\r', '\n']);
    let (immutable_container_id, labels_json) = text
        .split_once('\t')
        .ok_or(ReapFailure::IdentityUnavailable)?;
    if immutable_container_id.is_empty() || labels_json.contains('\t') {
        return Err(ReapFailure::IdentityUnavailable);
    }
    let labels: Option<std::collections::HashMap<String, String>> =
        serde_json::from_str(labels_json).map_err(|_| ReapFailure::IdentityUnavailable)?;
    let mut ownership_labels: Vec<_> = labels
        .unwrap_or_default()
        .into_iter()
        .filter(|(key, _)| key.starts_with("a2a."))
        .collect();
    ownership_labels.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(ContainerRuntimeIdentityV1 {
        immutable_container_id: immutable_container_id.to_owned(),
        ownership_labels,
    })
}

/// Parse a no-trunc runtime inventory and report exact selector presence.
///
/// Every non-empty row must be exactly two tab-separated fields, `ID<TAB>NAMES`. The ID is
/// non-empty. NAMES is either empty (a runtime can release an object's names before the object
/// leaves the inventory, so only the ID can match) or a comma-separated list with no empty or
/// bare-slash element. The whole inventory is validated before any answer, so a matching row never
/// hides a malformed one and a malformed row never lets the selector be reported absent.
pub fn parse_container_inventory_contains(
    bytes: &[u8],
    selector: &str,
) -> Result<bool, ReapFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| ReapFailure::IdentityUnavailable)?;
    let normalized_selector = selector.strip_prefix("sha256:").unwrap_or(selector);
    let mut present = false;
    for line in text.lines().filter(|line| !line.is_empty()) {
        let mut fields = line.split('\t');
        let (Some(container_id), Some(names), None) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(ReapFailure::IdentityUnavailable);
        };
        let normalized_id = container_id.strip_prefix("sha256:").unwrap_or(container_id);
        if normalized_id.is_empty() {
            return Err(ReapFailure::IdentityUnavailable);
        }
        present |= normalized_id == normalized_selector;
        if names.is_empty() {
            continue;
        }
        for name in names.split(',') {
            let name = name.strip_prefix('/').unwrap_or(name);
            if name.is_empty() {
                return Err(ReapFailure::IdentityUnavailable);
            }
            present |= name == selector;
        }
    }
    Ok(present)
}

async fn observe_container_identity(
    runtime: &str,
    selector: &str,
    timeout: Duration,
) -> Result<ContainerRuntimeIdentityV1, ReapFailure> {
    let mut command = tokio::process::Command::new(runtime);
    command
        .args([
            "container",
            "inspect",
            "--format",
            CONTAINER_IDENTITY_FORMAT,
            selector,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| ReapFailure::Spawn)?;
    let stdout = child.stdout.take().ok_or(ReapFailure::Spawn)?;
    let observation = async move {
        let mut bytes = Vec::new();
        stdout
            .take(CONTAINER_IDENTITY_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ReapFailure::IdentityUnavailable)?;
        let status = child.wait().await.map_err(|_| ReapFailure::Spawn)?;
        Ok((status.success(), bytes))
    };
    let (inspect_succeeded, bytes) = tokio::time::timeout(timeout, observation)
        .await
        .map_err(|_| ReapFailure::Timeout)??;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > CONTAINER_IDENTITY_MAX_BYTES {
        return Err(ReapFailure::IdentityUnavailable);
    }
    if inspect_succeeded {
        return parse_container_identity(&bytes);
    }
    if runtime_inventory_contains(runtime, selector, timeout).await? {
        Err(ReapFailure::IdentityUnavailable)
    } else {
        Err(ReapFailure::AlreadyGone)
    }
}

async fn runtime_inventory_contains(
    runtime: &str,
    selector: &str,
    timeout: Duration,
) -> Result<bool, ReapFailure> {
    let bound = tokio::time::Instant::now()
        .checked_add(timeout)
        .ok_or(ReapFailure::Timeout)?;
    runtime_inventory_contains_until(runtime, selector, bound).await
}

/// [`runtime_inventory_contains`] against an absolute `bound`, so the child's spawn, its output,
/// and its exit all share one caller-owned deadline.
async fn runtime_inventory_contains_until(
    runtime: &str,
    selector: &str,
    bound: tokio::time::Instant,
) -> Result<bool, ReapFailure> {
    let mut command = tokio::process::Command::new(runtime);
    command
        .args([
            "container",
            "ps",
            "--all",
            "--no-trunc",
            "--format",
            CONTAINER_INVENTORY_FORMAT,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| ReapFailure::Spawn)?;
    let stdout = child.stdout.take().ok_or(ReapFailure::Spawn)?;
    let observation = async move {
        let mut bytes = Vec::new();
        stdout
            .take(CONTAINER_IDENTITY_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ReapFailure::IdentityUnavailable)?;
        let status = child.wait().await.map_err(|_| ReapFailure::Spawn)?;
        if !status.success()
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > CONTAINER_IDENTITY_MAX_BYTES
        {
            return Err(ReapFailure::IdentityUnavailable);
        }
        parse_container_inventory_contains(&bytes, selector)
    };
    tokio::time::timeout_at(bound, observation)
        .await
        .map_err(|_| ReapFailure::Timeout)?
}

async fn remove_container_id(
    runtime: &str,
    immutable_container_id: &str,
    timeout: Duration,
) -> Result<(), ReapFailure> {
    // One absolute deadline bounds the removal child and every observation after it.
    let deadline = tokio::time::Instant::now()
        .checked_add(timeout)
        .ok_or(ReapFailure::Timeout)?;
    let (program, argv) = crate::sandbox::reap_argv(runtime, immutable_container_id);
    let mut command = tokio::process::Command::new(&program);
    command.args(&argv).kill_on_drop(true);
    let child = command.spawn().map_err(|_| ReapFailure::Spawn)?;
    // Neither exit status is evidence: `run --rm` can win the race behind a nonzero exit, and a
    // runtime wrapper can exit zero without removing anything. Only the inventory decides.
    tokio::time::timeout_at(deadline, child.wait_with_output())
        .await
        .map_err(|_| ReapFailure::Timeout)?
        .map_err(|_| ReapFailure::Spawn)?;
    confirm_exact_id_absent_after_removal(runtime, immutable_container_id, deadline).await
}

/// Decide whether the one exact-ID `rm -f` achieved absence. Docker's `run --rm` can already be
/// removing the container, so the runtime refuses the loser of that race even though the captured
/// object is going away. The ID being absent from a complete, in-bounds runtime inventory is the
/// only evidence that settles cleanup as success, whatever the exit status was. There is never a
/// second removal: a present object is only re-observed, briefly, in case the removal in progress
/// finishes.
///
/// Every inventory child is bounded by one absolute `observe_until`, the earlier of the removal's
/// original `deadline` and the observation window, and an absence first seen after that bound is
/// refused. Reaching the bound without accepted absence is `NonZeroExit` if the object was ever
/// seen present (the removal did not achieve absence) and `Timeout` if it never was. An unusable
/// inventory fails closed without a retry and keeps its own typed cause (`Spawn` or
/// `IdentityUnavailable`), because durable recovery classifies on it.
async fn confirm_exact_id_absent_after_removal(
    runtime: &str,
    immutable_container_id: &str,
    deadline: tokio::time::Instant,
) -> Result<(), ReapFailure> {
    let observe_until = tokio::time::Instant::now()
        .checked_add(CONTAINER_REMOVAL_OBSERVATION_WINDOW)
        .map_or(deadline, |end| end.min(deadline));
    let mut observed_present = false;
    loop {
        let unsettled = if observed_present {
            ReapFailure::NonZeroExit
        } else {
            ReapFailure::Timeout
        };
        if tokio::time::Instant::now() >= observe_until {
            return Err(unsettled);
        }
        let contained =
            match runtime_inventory_contains_until(runtime, immutable_container_id, observe_until)
                .await
            {
                Ok(contained) => contained,
                Err(ReapFailure::Timeout) => return Err(unsettled),
                Err(error) => return Err(error),
            };
        match contained {
            // `timeout_at` polls its inner future first, so a stale answer can arrive after the
            // bound; absence is only accepted if it is still inside it.
            false if tokio::time::Instant::now() < observe_until => return Ok(()),
            false => return Err(unsettled),
            true => observed_present = true,
        }
        if tokio::time::Instant::now() + CONTAINER_REMOVAL_OBSERVATION_INTERVAL >= observe_until {
            return Err(ReapFailure::NonZeroExit);
        }
        tokio::time::sleep(CONTAINER_REMOVAL_OBSERVATION_INTERVAL).await;
    }
}

fn classify_container_start_status(status: &[u8]) -> ContainerStartState {
    let Ok(status) = std::str::from_utf8(status) else {
        return ContainerStartState::Unknown;
    };
    match status.trim() {
        // Docker reports `created`; Podman may expose the adjacent `configured`/`initialized` states.
        "created" | "configured" | "initialized" => ContainerStartState::NotStarted,
        "running" | "restarting" | "paused" | "exited" | "stopped" | "dead" | "removing"
        | "stopping" => ContainerStartState::Started,
        _ => ContainerStartState::Unknown,
    }
}

fn production_start_probe(timeout: Duration) -> ContainerStartProbeFn {
    Arc::new(move |runtime, name| {
        Box::pin(async move {
            let mut command = tokio::process::Command::new(&runtime);
            command
                .args([
                    "container",
                    "inspect",
                    "--format",
                    "{{.State.Status}}",
                    &name,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(_) => return ContainerStartState::Unknown,
            };
            let Some(stdout) = child.stdout.take() else {
                return ContainerStartState::Unknown;
            };
            let observation = async move {
                let mut bytes = Vec::new();
                stdout
                    .take(CONTAINER_START_STATUS_MAX_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| ())?;
                let status = child.wait().await.map_err(|_| ())?;
                if !status.success()
                    || u64::try_from(bytes.len()).unwrap_or(u64::MAX)
                        > CONTAINER_START_STATUS_MAX_BYTES
                {
                    return Ok(ContainerStartState::Unknown);
                }
                Ok(classify_container_start_status(&bytes))
            };
            match tokio::time::timeout(timeout, observation).await {
                Ok(Ok(state)) => state,
                Ok(Err(())) | Err(_) => ContainerStartState::Unknown,
            }
        })
    })
}

/// Reap a named container exactly once (idempotent via the shared `reaped` flag).
pub fn reap_once(reap_fn: &ReapFn, runtime: &str, name: &str, reaped: &Arc<AtomicBool>) {
    if !reaped.swap(true, Ordering::SeqCst) {
        reap_fn(runtime.to_string(), name.to_string());
    }
}

/// Spawn a future onto the current runtime if there is one, else on a throwaway thread+runtime. `Drop`
/// can fire off-runtime (process shutdown), so this must never panic.
pub fn spawn_detached<F: Future<Output = ()> + Send + 'static>(fut: F) {
    match tokio::runtime::Handle::try_current() {
        Ok(h) => {
            h.spawn(fut);
        }
        Err(_) => {
            std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    rt.block_on(fut);
                }
            });
        }
    }
}

/// Compatibility reaper. It resolves the selector to an immutable ID first;
/// the destructive runtime call never receives a container name.
pub fn production_reap_fn() -> ReapFn {
    Arc::new(|runtime: String, selector: String| {
        spawn_detached(async move {
            match observe_container_identity(&runtime, &selector, Duration::from_secs(2)).await {
                Ok(observed) => {
                    if let Err(error) = remove_container_id(
                        &runtime,
                        &observed.immutable_container_id,
                        Duration::from_secs(10),
                    )
                    .await
                    {
                        tracing::warn!(
                            runtime,
                            selector,
                            code = error.code(),
                            "container reap failed"
                        );
                    }
                }
                Err(ReapFailure::AlreadyGone) => {}
                Err(error) => {
                    tracing::warn!(
                        runtime,
                        selector,
                        code = error.code(),
                        "container identity could not be resolved for reap"
                    );
                }
            }
        });
    })
}

// ---- Increment A: label-scoped reaping + liveness sweep + staleness (shell out; live-gated) -------------

/// Reap THIS run's containers (END-sweep): `ps -aq --filter label=a2a.run=<id>` → `rm -f` each. Best-effort.
pub fn run_scoped_reap(runtime: &str, run_id: &str) {
    let (p, argv) = crate::sandbox::by_run_filter_argv(runtime, run_id);
    if let Ok(out) = std::process::Command::new(&p).args(&argv).output() {
        for id in String::from_utf8_lossy(&out.stdout).split_whitespace() {
            let _ = std::process::Command::new(runtime)
                .args(["rm", "-f", id])
                .output();
        }
    }
}

/// PURE recovery planner: classify a batch of inspected managed records `(id, host, lease)` against the
/// CURRENT lease state and return `(reap_ids, dead_leases)` — the container ids to `rm -f` (DEAD only:
/// same host + a FREE lease lock) and the DISTINCT dead lease files (order-preserving, deduped).
///
/// Classifying the WHOLE batch BEFORE any lease file is removed is the correctness keystone: a crashed run's
/// containers span MULTIPLE owners (e.g. a `:rw` implementor + per-reviewer `:ro` readers) but share ONE
/// lease file. If a sibling reap deleted that lease mid-recovery, every later owner's sweep would probe an
/// ABSENT lease → [`crate::run_identity::classify`] returns `Unknown` → spared → the orphan LEAKS (live-gate
/// finding). So lease DELETION is the caller's job, performed ONCE after EVERY owner has been swept.
pub fn plan_recovery(
    records: &[(String, String, String)],
    my_host: &str,
    probe: &dyn crate::liveness::LeaseProbe,
) -> (Vec<String>, Vec<String>) {
    use crate::run_identity::{classify, Verdict};
    let mut reap_ids = Vec::new();
    let mut dead_leases: Vec<String> = Vec::new();
    for (id, host, lease) in records {
        let labels = std::collections::HashMap::from([
            ("a2a.host".to_string(), host.clone()),
            ("a2a.lease".to_string(), lease.clone()),
        ]);
        if classify(&labels, my_host, probe) == Verdict::Dead {
            reap_ids.push(id.clone());
            if !lease.is_empty() && !dead_leases.contains(lease) {
                dead_leases.push(lease.clone());
            }
        }
    }
    (reap_ids, dead_leases)
}

/// Owner-scoped crash-recovery: inspect each MANAGED container in `owner`, [`plan_recovery`] the batch, and
/// reap ONLY `Dead` (same host + free lease lock). RETURNS the dead lease files (deduped) for the caller to
/// delete ONCE, after EVERY owner has been swept — NOT here: a crashed run's containers span multiple owners
/// but share one lease, so deleting it per-owner would blind the later owners' sweeps (see [`plan_recovery`]).
/// Never touches Alive/Unknown. Best-effort (any docker error ⇒ no reaps, no dead leases).
#[must_use]
pub fn classify_sweep(
    runtime: &str,
    owner: &str,
    my_host: &str,
    probe: &dyn crate::liveness::LeaseProbe,
) -> Vec<String> {
    let (p, argv) = crate::sandbox::managed_inspect_argv(runtime, owner);
    let Ok(out) = std::process::Command::new(&p).args(&argv).output() else {
        return Vec::new();
    };
    let records: Vec<(String, String, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut it = line.split('\t');
            match (it.next(), it.next(), it.next()) {
                (Some(id), Some(host), Some(lease)) => {
                    Some((id.to_string(), host.to_string(), lease.to_string()))
                }
                _ => None,
            }
        })
        .collect();
    let (reap_ids, dead_leases) = plan_recovery(&records, my_host, probe);
    for id in reap_ids {
        let _ = std::process::Command::new(runtime)
            .args(["rm", "-f", &id])
            .output();
    }
    dead_leases
}

/// True iff the container produced NO log line within `window` (⇒ stale). `docker logs --since <window>
/// --tail 1 <name>` empty ⇒ stale. Best-effort: any error ⇒ false (bias against a false-stale flag).
pub fn is_stale(runtime: &str, name: &str, window: &str) -> bool {
    match std::process::Command::new(runtime)
        .args(["logs", "--since", window, "--tail", "1", name])
        .output()
    {
        Ok(o) => o.stdout.is_empty() && o.stderr.is_empty(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retained_resource_flight::{
        FileResourceFlightJournal, ResourceFlightJournalError, ResourceFlightJournalEventV1,
        ResourceFlightJournalRecordV1, ResourceFlightReservationOutcomeV1,
        ResourceFlightReservationRecordV1, ResourceFlightTerminalAppendOutcomeV1,
    };
    use std::sync::atomic::AtomicUsize;

    const SUCCESSFUL_RUNTIME_TEST_TIMEOUT: Duration = Duration::from_secs(5);

    #[cfg(unix)]
    static PRODUCTION_RUNTIME_FIXTURE_PERMIT: tokio::sync::Semaphore =
        tokio::sync::Semaphore::const_new(1);

    /// Execute a just-written fake runtime once with no arguments, retrying the `ETXTBSY`
    /// fork/exec race (see `custody_git_tests::retry_on_text_file_busy`), before the production
    /// controller under test executes it. One exec that is not refused proves no writer remains.
    #[cfg(unix)]
    fn warm_up_runtime(path: &std::path::Path) {
        crate::custody_git_tests::retry_on_text_file_busy(|| {
            std::process::Command::new(path).output()
        })
        .expect("warm-up exec of the fake runtime");
    }

    /// Linux regression control for [`warm_up_runtime`]: hold a writable descriptor on a
    /// just-written fake runtime, the way a concurrently forked child does, and release it from a
    /// detached thread only after 100 ms. Nothing waits for that release except the warm-up's
    /// bounded retry, so without the warm-up the production call that follows meets the held
    /// writer and its spawn is refused. macOS executes a written `#!` script even while a writer
    /// is held, so nothing is held there.
    #[cfg(target_os = "linux")]
    fn hold_writer_briefly(path: &std::path::Path) {
        let writer = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("hold a writer on the fake runtime");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(writer);
        });
    }

    /// Warm up a just-written fake runtime; on Linux, first hold a writer that only the warm-up's
    /// bounded retry can outlast (see [`hold_writer_briefly`]).
    #[cfg(unix)]
    fn warm_up_runtime_through_a_held_writer(path: &std::path::Path) {
        #[cfg(target_os = "linux")]
        hold_writer_briefly(path);
        warm_up_runtime(path);
    }

    #[cfg(unix)]
    async fn production_runtime_fixture_permit() -> tokio::sync::SemaphorePermit<'static> {
        PRODUCTION_RUNTIME_FIXTURE_PERMIT
            .acquire()
            .await
            .expect("production-runtime fixture semaphore is never closed")
    }

    #[derive(Default)]
    struct RecordingPublisher(
        StdMutex<Vec<crate::retained_resource_flight::NodeCleanupAggregationV1>>,
    );
    impl ResourceFlightResultPublisher for RecordingPublisher {
        fn publish(&self, aggregation: crate::retained_resource_flight::NodeCleanupAggregationV1) {
            self.0.lock().unwrap().push(aggregation);
        }
    }

    #[derive(Clone, Copy)]
    enum ContainerJournalFault {
        Dispatch,
        RemovalObservation,
        BlockCompleteTerminal,
    }

    struct FaultContainerJournal {
        inner: InMemoryResourceFlightJournal,
        fault: ContainerJournalFault,
        driver_at_terminal: Option<std::sync::Barrier>,
        recovery_done: Option<std::sync::Barrier>,
    }

    impl FaultContainerJournal {
        fn new(fault: ContainerJournalFault) -> Self {
            let blocks = matches!(fault, ContainerJournalFault::BlockCompleteTerminal);
            Self {
                inner: InMemoryResourceFlightJournal::new(512),
                fault,
                driver_at_terminal: blocks.then(|| std::sync::Barrier::new(2)),
                recovery_done: blocks.then(|| std::sync::Barrier::new(2)),
            }
        }
    }

    impl ResourceFlightJournal for FaultContainerJournal {
        fn reserve_flight(
            &self,
            reservation: &ResourceFlightReservationRecordV1,
        ) -> Result<ResourceFlightReservationOutcomeV1, ResourceFlightJournalError> {
            self.inner.reserve_flight(reservation)
        }

        fn reservations(
            &self,
        ) -> Result<Vec<ResourceFlightReservationRecordV1>, ResourceFlightJournalError> {
            self.inner.reservations()
        }

        fn rollback_empty_reservation(
            &self,
            reservation: &ResourceFlightReservationRecordV1,
        ) -> Result<bool, ResourceFlightJournalError> {
            self.inner.rollback_empty_reservation(reservation)
        }

        fn append(
            &self,
            id: &ResourceFlightIdV1,
            record: &ResourceFlightJournalRecordV1,
        ) -> Result<(), ResourceFlightJournalError> {
            let fail = match self.fault {
                ContainerJournalFault::Dispatch => {
                    matches!(
                        &record.event,
                        ResourceFlightJournalEventV1::DispatchStarted {}
                    )
                }
                ContainerJournalFault::RemovalObservation => matches!(
                    &record.event,
                    ResourceFlightJournalEventV1::ContainerRemovalObserved { .. }
                ),
                ContainerJournalFault::BlockCompleteTerminal => false,
            };
            if fail {
                return Err(ResourceFlightJournalError::Accounting);
            }
            self.inner.append(id, record)
        }

        fn append_terminal(
            &self,
            id: &ResourceFlightIdV1,
            result: &ResourceActionResultV1,
        ) -> Result<ResourceFlightTerminalAppendOutcomeV1, ResourceFlightJournalError> {
            if matches!(self.fault, ContainerJournalFault::BlockCompleteTerminal)
                && result.disposition == ResourceActionDispositionV1::Complete
            {
                self.driver_at_terminal.as_ref().unwrap().wait();
                self.recovery_done.as_ref().unwrap().wait();
            }
            self.inner.append_terminal(id, result)
        }

        fn records(
            &self,
            id: &ResourceFlightIdV1,
        ) -> Result<Vec<ResourceFlightJournalRecordV1>, ResourceFlightJournalError> {
            self.inner.records(id)
        }
    }

    fn durable_container_for_test(
        identity: &ResourceIdentityV1,
        journal: Arc<dyn ResourceFlightJournal>,
    ) -> (DurableContainerFlightV3, ResourceFlightIdV1) {
        let ResourceIdentityV1::ManagedContainer { generation, .. } = identity else {
            panic!("expected managed container identity");
        };
        let attempt_id = AttemptId::mint().unwrap();
        let flight_id = ResourceFlightIdV1::mint().unwrap();
        (
            DurableContainerFlightV3 {
                attempt_id: attempt_id.clone(),
                generation: generation.clone(),
                flight_id: flight_id.clone(),
                registry: Arc::new(ResourceFlightRegistryV1::new(attempt_id)),
                journal,
                owner: ResourceFlightOwnerV1::new(
                    NodeId::parse("container-spawn").unwrap(),
                    generation,
                )
                .unwrap(),
                result_publisher: Arc::new(NoopResourceFlightResultPublisher),
            },
            flight_id,
        )
    }

    #[test]
    fn reap_once_fires_exactly_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let reap_fn: ReapFn = Arc::new(move |_r, _n| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        let reaped = Arc::new(AtomicBool::new(false));
        reap_once(&reap_fn, "docker", "a2a-ro-x", &reaped);
        reap_once(&reap_fn, "docker", "a2a-ro-x", &reaped); // 2nd call no-ops
        reap_once(&reap_fn, "docker", "a2a-ro-x", &reaped);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn spawn_detached_off_runtime_does_not_panic() {
        // Called from a plain thread (no tokio runtime) — the Drop-at-shutdown case.
        let done = Arc::new(AtomicBool::new(false));
        let d = Arc::clone(&done);
        std::thread::spawn(move || {
            spawn_detached(async move {
                d.store(true, Ordering::SeqCst);
            });
        })
        .join()
        .unwrap();
        // No panic = pass (the detached work runs on its own thread; we don't join it).
    }

    #[test]
    fn identity_template_and_parser_use_one_real_tab_boundary() {
        assert_eq!(
            CONTAINER_IDENTITY_FORMAT,
            r#"{{.Id}}{{"\t"}}{{json .Config.Labels}}"#
        );
        let parsed = parse_container_identity(
            b"sha256:immutable\t{\"a2a.owner\":\"owner\",\"image.label\":\"ignored\",\"a2a.future\":\"extra\"}\n",
        )
        .unwrap();
        assert_eq!(parsed.immutable_container_id, "sha256:immutable");
        assert_eq!(
            parsed.ownership_labels,
            vec![
                ("a2a.future".into(), "extra".into()),
                ("a2a.owner".into(), "owner".into()),
            ]
        );
        assert_eq!(
            parse_container_identity(b"sha256:immutable\\t{\"a2a.owner\":\"owner\"}\n"),
            Err(ReapFailure::IdentityUnavailable),
            "a literal backslash-t is not the format contract"
        );
    }

    #[test]
    fn runtime_inventory_parser_matches_exact_ids_and_names() {
        let inventory = b"sha256:one\talpha,beta\ntwo\t/stable-name\n";
        assert_eq!(
            parse_container_inventory_contains(inventory, "sha256:one"),
            Ok(true)
        );
        assert_eq!(
            parse_container_inventory_contains(inventory, "stable-name"),
            Ok(true)
        );
        assert_eq!(
            parse_container_inventory_contains(inventory, "stable"),
            Ok(false)
        );
        assert_eq!(
            parse_container_inventory_contains(b"malformed\n", "stable-name"),
            Err(ReapFailure::IdentityUnavailable)
        );
    }

    /// One `ID<TAB>NAMES` inventory row per way a row can be malformed (each is a single line, no
    /// newline). No runtime prints any of them, so none may be read as "this object is absent" or
    /// hidden behind a row that matches the selector.
    const MALFORMED_INVENTORY_ROWS: &[(&str, &str)] = &[
        ("missing tab", "other"),
        ("extra field", "other\tname\textra"),
        ("trailing tab after names", "other\tname\t"),
        ("empty middle field", "other\t\tname"),
        ("many fields", "other\ta\tb\tc"),
        ("empty names and a trailing tab", "other\t\t"),
        ("empty id", "\tname"),
        ("empty id and names", "\t"),
        ("bare sha256 id", "sha256:\tname"),
        ("empty name in the list", "other\ta,,b"),
        ("leading empty name", "other\t,a"),
        ("trailing empty name", "other\ta,"),
        ("only a comma", "other\t,"),
        ("bare slash name", "other\t/"),
        ("bare slash after a name", "other\ta,/"),
    ];

    #[test]
    fn runtime_inventory_parser_refuses_every_malformed_row_wherever_it_appears() {
        let id = "sha256:aaaa";
        let matching = format!("{id}\tstable-name\n");
        let mut accepted = Vec::new();
        for (label, row) in MALFORMED_INVENTORY_ROWS {
            for (position, inventory, selector) in [
                ("alone", format!("{row}\n"), id),
                ("after an unrelated row", format!("x\ty\n{row}\n"), id),
                ("before the ID match", format!("{row}\n{matching}"), id),
                ("after the ID match", format!("{matching}{row}\n"), id),
                (
                    "after the name match",
                    format!("{matching}{row}\n"),
                    "stable-name",
                ),
                (
                    "last, with no final newline",
                    format!("{matching}{row}"),
                    id,
                ),
            ] {
                let result = parse_container_inventory_contains(inventory.as_bytes(), selector);
                if result != Err(ReapFailure::IdentityUnavailable) {
                    accepted.push(format!("{label}, {position}: {result:?}"));
                }
            }
        }
        assert!(
            accepted.is_empty(),
            "malformed rows must refuse the whole inventory:\n{}",
            accepted.join("\n")
        );
    }

    #[test]
    fn runtime_inventory_parser_refuses_invalid_utf8_anywhere() {
        for inventory in [
            b"\xff\n".as_slice(),
            b"sha256:one\talpha\n\xff\n",
            b"\xffsha256:one\talpha\n",
            b"sha256:one\talp\xffha\n",
        ] {
            for selector in ["sha256:one", "alpha", "absent"] {
                assert_eq!(
                    parse_container_inventory_contains(inventory, selector),
                    Err(ReapFailure::IdentityUnavailable),
                    "{inventory:?} / {selector}"
                );
            }
        }
    }

    #[test]
    fn runtime_inventory_parser_keeps_valid_presence_and_absence() {
        // Blank lines are not rows; CRLF and a leading slash on each name are runtime spellings.
        let inventory = b"\nsha256:one\t/alpha,/beta\n\ntwo\t/stable-name\r\nthree\tgamma\n";
        for (selector, expected) in [
            ("sha256:one", true),
            ("one", true),
            ("alpha", true),
            ("beta", true),
            ("two", true),
            ("stable-name", true),
            ("gamma", true),
            ("three", true),
            ("stable", false),
            ("sha256:four", false),
            ("onee", false),
        ] {
            assert_eq!(
                parse_container_inventory_contains(inventory, selector),
                Ok(expected),
                "{selector}"
            );
        }
        for empty in [b"".as_slice(), b"\n", b"\n\n"] {
            assert_eq!(
                parse_container_inventory_contains(empty, "sha256:one"),
                Ok(false)
            );
        }
    }

    #[test]
    fn runtime_inventory_parser_allows_an_empty_names_field_that_only_the_id_can_match() {
        // A runtime can release an object's names before the object leaves the inventory. The ID
        // still identifies it, and no name alias exists to match.
        let inventory = b"sha256:three\t\nother\t/kept\n";
        for (selector, expected) in [
            ("sha256:three", true),
            ("three", true),
            ("kept", true),
            ("absent", false),
            // With no names, not even an empty selector has an alias to match.
            ("", false),
        ] {
            assert_eq!(
                parse_container_inventory_contains(inventory, selector),
                Ok(expected),
                "{selector:?}"
            );
        }
        assert_eq!(
            parse_container_inventory_contains(b"three\t\n", "other"),
            Ok(false),
            "an empty names field is a complete row that names nothing else"
        );
    }

    #[test]
    fn container_start_status_classification_is_closed() {
        for status in [b"created".as_slice(), b"configured", b"initialized"] {
            assert_eq!(
                classify_container_start_status(status),
                ContainerStartState::NotStarted
            );
        }
        for status in [
            b"running".as_slice(),
            b"restarting",
            b"paused",
            b"exited",
            b"stopped",
            b"dead",
            b"removing",
            b"stopping",
        ] {
            assert_eq!(
                classify_container_start_status(status),
                ContainerStartState::Started
            );
        }
        for status in [
            b"".as_slice(),
            b"unknown",
            b"CREATED",
            b"created extra",
            &[0xff],
        ] {
            assert_eq!(
                classify_container_start_status(status),
                ContainerStartState::Unknown
            );
        }
    }

    #[tokio::test]
    async fn panicking_start_probes_fail_closed_to_unknown() {
        let attempt: ReapAttemptFn = Arc::new(|_runtime, _name| Box::pin(async move { Ok(()) }));
        let synchronous: ContainerStartProbeFn =
            Arc::new(|_runtime, _name| panic!("synchronous start probe panic"));
        let controller = ReapController::new("docker", "a2a-ro-sync-panic", Arc::clone(&attempt))
            .with_start_probe(synchronous);
        assert_eq!(
            controller.probe_start_state().await,
            ContainerStartState::Unknown
        );

        let asynchronous: ContainerStartProbeFn = Arc::new(|_runtime, _name| {
            Box::pin(async move { panic!("asynchronous start probe panic") })
        });
        let controller = ReapController::new("docker", "a2a-ro-async-panic", attempt)
            .with_start_probe(asynchronous);
        assert_eq!(
            controller.probe_start_state().await,
            ContainerStartState::Unknown
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn production_start_probe_is_bounded_and_requires_exact_status() {
        use std::os::unix::fs::PermissionsExt;

        let _fixture_permit = production_runtime_fixture_permit().await;
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        std::fs::write(
            &runtime,
            "#!/bin/sh\ncase \"$5\" in\n  created) printf created ;;\n  running) printf running ;;\n  oversized) printf '%065d' 0 ;;\n  *) exit 1 ;;\nesac\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&runtime, permissions).unwrap();
        warm_up_runtime(&runtime);
        // Match the production bound for ordinary exact-status observations. The separate hung-runtime
        // control below keeps its deliberately short timeout and proves cancellation independently.
        let probe = production_start_probe(CONTAINER_START_PROBE_TIMEOUT);
        let runtime = runtime.to_string_lossy().into_owned();

        assert_eq!(
            probe(runtime.clone(), "created".into()).await,
            ContainerStartState::NotStarted
        );
        assert_eq!(
            probe(runtime.clone(), "running".into()).await,
            ContainerStartState::Started
        );
        assert_eq!(
            probe(runtime.clone(), "oversized".into()).await,
            ContainerStartState::Unknown
        );
        assert_eq!(
            probe(runtime, "missing".into()).await,
            ContainerStartState::Unknown
        );

        // The no-argument branch is the side-effect-free warm-up. The probe's own invocation
        // records that it started, then blocks until released, so only the timeout can end the
        // probe. A refused spawn also returns `Unknown`, but never records a start.
        let hung_runtime = temp.path().join("hung-runtime");
        let marker = temp.path().join("late-side-effect");
        let started = temp.path().join("late-side-effect.started");
        let release = temp.path().join("late-side-effect.release");
        std::fs::write(
            &hung_runtime,
            "#!/bin/sh\n[ \"$#\" -eq 0 ] && exit 0\n: > \"$5.started\"\nwhile [ ! -e \"$5.release\" ]; do sleep 0.01; done\nprintf reached > \"$5\"\nprintf running\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&hung_runtime).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hung_runtime, permissions).unwrap();
        warm_up_runtime_through_a_held_writer(&hung_runtime);
        // Long enough for the started child to record its start before the timeout kills it.
        let probe = production_start_probe(Duration::from_secs(1));
        assert_eq!(
            probe(
                hung_runtime.to_string_lossy().into_owned(),
                marker.to_string_lossy().into_owned(),
            )
            .await,
            ContainerStartState::Unknown
        );
        assert!(
            started.exists(),
            "the timed-out probe must have started the runtime, not failed to spawn it"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::write(release, b"release").unwrap();
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(
            !marker.exists(),
            "a timed-out runtime probe must not reach its delayed side effect"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn production_runtime_fixture_permit_serializes_and_allows_execution() {
        use std::os::unix::fs::PermissionsExt;

        let first_permit = production_runtime_fixture_permit().await;
        let second_permit = production_runtime_fixture_permit();
        tokio::pin!(second_permit);
        assert!(matches!(
            futures::poll!(second_permit.as_mut()),
            std::task::Poll::Pending
        ));
        drop(first_permit);
        let _second_permit = second_permit.await;

        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        std::fs::write(&runtime, "#!/bin/sh\nprintf created\n").unwrap();
        let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&runtime, permissions).unwrap();
        warm_up_runtime(&runtime);

        let runtime = runtime.to_string_lossy().into_owned();
        let probe = production_start_probe(SUCCESSFUL_RUNTIME_TEST_TIMEOUT);
        assert_eq!(
            probe(runtime, "serialized-fixture".into()).await,
            ContainerStartState::NotStarted
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn missing_container_completes_for_docker_and_podman_without_stderr_matching() {
        use std::os::unix::fs::PermissionsExt;

        let _fixture_permit = production_runtime_fixture_permit().await;
        let temp = tempfile::tempdir().unwrap();
        for runtime_name in ["docker", "podman"] {
            let runtime = temp.path().join(runtime_name);
            let marker = temp.path().join(format!("{runtime_name}-removed"));
            std::fs::write(
                &runtime,
                format!(
                    "#!/bin/sh\nif [ \"$1\" = container ] && [ \"$2\" = inspect ]; then printf 'opaque localized failure' >&2; exit 1; fi\nif [ \"$1\" = container ] && [ \"$2\" = ps ]; then exit 0; fi\nif [ \"$1\" = rm ]; then printf called > '{}'; exit 0; fi\nexit 2\n",
                    marker.display()
                ),
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&runtime, permissions).unwrap();
            warm_up_runtime(&runtime);

            let controller = ReapController::production_with_timeout(
                runtime.to_string_lossy(),
                "missing-name",
                SUCCESSFUL_RUNTIME_TEST_TIMEOUT,
            );
            assert_eq!(controller.reap_observed().await, Ok(()));
            assert!(
                !marker.exists(),
                "absence must not dispatch rm for {runtime_name}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_inventory_keeps_missing_selector_classification_unknown() {
        use std::os::unix::fs::PermissionsExt;

        let _fixture_permit = production_runtime_fixture_permit().await;
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        std::fs::write(
            &runtime,
            "#!/bin/sh\nif [ \"$1\" = container ] && [ \"$2\" = inspect ]; then exit 1; fi\nif [ \"$1\" = container ] && [ \"$2\" = ps ]; then exit 2; fi\nexit 3\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&runtime, permissions).unwrap();
        warm_up_runtime(&runtime);

        let controller = ReapController::production_with_timeout(
            runtime.to_string_lossy(),
            "unresolved-name",
            SUCCESSFUL_RUNTIME_TEST_TIMEOUT,
        );
        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::IdentityUnavailable)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_malformed_inventory_keeps_missing_selector_classification_unknown() {
        use std::os::unix::fs::PermissionsExt;

        let _fixture_permit = production_runtime_fixture_permit().await;
        let mut wrong = Vec::new();
        for (label, row) in MALFORMED_INVENTORY_ROWS {
            let temp = tempfile::tempdir().unwrap();
            let runtime = temp.path().join("runtime");
            let dir = temp.path().display();
            std::fs::write(
                &runtime,
                format!(
                    "#!/bin/sh\nif [ \"$1\" = container ] && [ \"$2\" = inspect ]; then printf x >> '{dir}/inspect.log'; exit 1; fi\nif [ \"$1\" = container ] && [ \"$2\" = ps ]; then printf x >> '{dir}/ps.log'; printf '%s\\n' '{row}'; exit 0; fi\nif [ \"$1\" = rm ]; then printf called > '{dir}/removed'; exit 0; fi\nexit 2\n"
                ),
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&runtime, permissions).unwrap();
            warm_up_runtime(&runtime);

            let controller = ReapController::production_with_timeout(
                runtime.to_string_lossy(),
                "unresolved-name",
                SUCCESSFUL_RUNTIME_TEST_TIMEOUT,
            );
            let result = controller.reap_observed().await;

            assert!(
                temp.path().join("inspect.log").exists() && temp.path().join("ps.log").exists(),
                "{label}: the identity probe must fall through to the inventory"
            );
            if result != Err(ReapFailure::IdentityUnavailable)
                || temp.path().join("removed").exists()
            {
                wrong.push(format!("{label}: {result:?}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "a malformed inventory must never classify a selector as already gone:\n{}",
            wrong.join("\n")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ro_production_controller_spares_a_recycled_name() {
        use std::os::unix::fs::PermissionsExt;

        let _fixture_permit = production_runtime_fixture_permit().await;
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let successor = temp.path().join("successor");
        let removed = temp.path().join("removed");
        std::fs::write(
            &runtime,
            format!(
                "#!/bin/sh\nif [ \"$1\" = container ] && [ \"$2\" = inspect ]; then\n  if [ -e '{}' ]; then printf 'new-id\\t{{\"a2a.owner\":\"owner\"}}\\n'; else printf 'old-id\\t{{\"a2a.owner\":\"owner\"}}\\n'; fi\n  exit 0\nfi\nif [ \"$1\" = rm ]; then printf called > '{}'; exit 0; fi\nexit 2\n",
                successor.display(),
                removed.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&runtime, permissions).unwrap();
        warm_up_runtime(&runtime);

        let controller = ReapController::production_with_timeout(
            runtime.to_string_lossy(),
            "stable-name",
            SUCCESSFUL_RUNTIME_TEST_TIMEOUT,
        );
        assert_eq!(controller.capture_production_identity().await, Ok(()));
        std::fs::write(&successor, b"ready").unwrap();
        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::IdentityChanged)
        );
        assert!(
            !removed.exists(),
            "a recycled selector must never reach exact-ID removal"
        );
    }

    /// The container ID every removal-race fixture captures and removes.
    #[cfg(unix)]
    const RACE_CONTAINER_ID: &str =
        "3f5a0c9e7b1d4a62c8e0f9137d5b2a4c6e8f01a3b5d7c9e2f4a6b8c0d1e3f5a7";

    /// A fake runtime for the Docker `run --rm` versus `rm -f` removal race. `inspect` reports
    /// [`RACE_CONTAINER_ID`]; each `rm` and `container ps` call is recorded so a test can prove the
    /// production call started and how many times each command ran. `rm_body` and `ps_body` are the
    /// shell bodies of those two branches; `$dir`, `$id`, and (in `ps_body`) `$n`, the 1-based `ps`
    /// call number, are in scope.
    #[cfg(unix)]
    struct RemovalRaceRuntime {
        temp: tempfile::TempDir,
        runtime: std::path::PathBuf,
    }

    #[cfg(unix)]
    impl RemovalRaceRuntime {
        fn new(rm_body: &str, ps_body: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;

            let temp = tempfile::tempdir().unwrap();
            let runtime = temp.path().join("runtime");
            std::fs::write(
                &runtime,
                format!(
                    "#!/bin/sh\ndir='{dir}'\nid='{id}'\n\
                     if [ \"$1\" = container ] && [ \"$2\" = inspect ]; then\n\
                     printf '%s\\t{{\"a2a.owner\":\"owner\"}}\\n' \"$id\"\nexit 0\nfi\n\
                     if [ \"$1\" = rm ]; then\nprintf '%s\\n' \"$*\" >> \"$dir/rm.log\"\n{rm_body}\nfi\n\
                     if [ \"$1\" = container ] && [ \"$2\" = ps ]; then\n\
                     printf x >> \"$dir/ps.log\"\nn=$(wc -c < \"$dir/ps.log\" | tr -d ' ')\n{ps_body}\nfi\n\
                     exit 2\n",
                    dir = temp.path().display(),
                    id = RACE_CONTAINER_ID,
                ),
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&runtime, permissions).unwrap();
            warm_up_runtime(&runtime);
            Self { temp, runtime }
        }

        fn runtime(&self) -> String {
            self.runtime.to_string_lossy().into_owned()
        }

        /// The recorded `rm` argument vectors, one line each.
        fn rm_calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.temp.path().join("rm.log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        /// The number of `container ps` inventory calls.
        fn ps_calls(&self) -> usize {
            std::fs::read(self.temp.path().join("ps.log")).map_or(0, |bytes| bytes.len())
        }

        /// A production controller with the identity captured, ready to run the exact-ID removal.
        async fn controller(&self, timeout: Duration) -> ReapController {
            let controller =
                ReapController::production_with_timeout(self.runtime(), "stable-name", timeout);
            assert_eq!(controller.capture_production_identity().await, Ok(()));
            controller
        }
    }

    /// Docker's answer when `run --rm` is already removing the container: nonzero, nothing else
    /// the bridge may rely on.
    #[cfg(unix)]
    const RM_ALREADY_IN_PROGRESS: &str = "exit 1";

    /// A runtime whose `rm -f` reports success; the tests never treat that as proof of absence.
    #[cfg(unix)]
    const RM_SUCCEEDS: &str = "exit 0";

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_rm_with_the_exact_id_absent_settles_cleanup_successfully() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        let fixture = RemovalRaceRuntime::new(RM_ALREADY_IN_PROGRESS, "exit 0");

        let controller = fixture.controller(SUCCESSFUL_RUNTIME_TEST_TIMEOUT).await;
        let result = controller.reap_observed().await;

        assert_eq!(
            fixture.rm_calls(),
            [format!("rm -f {RACE_CONTAINER_ID}")],
            "the production removal must start and run exactly once, on the captured ID"
        );
        assert_eq!(result, Ok(()));
        assert_eq!(fixture.ps_calls(), 1, "absence is proven by one inventory");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_rm_with_delayed_absence_settles_within_the_observation_window() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        // The removal stays in progress for the first two inventories, then completes.
        let fixture = RemovalRaceRuntime::new(
            RM_ALREADY_IN_PROGRESS,
            "if [ \"$n\" -le 2 ]; then printf '%s\\tstable-name\\n' \"$id\"; fi\nexit 0",
        );

        let controller = fixture.controller(SUCCESSFUL_RUNTIME_TEST_TIMEOUT).await;
        let result = controller.reap_observed().await;

        assert_eq!(
            fixture.rm_calls(),
            [format!("rm -f {RACE_CONTAINER_ID}")],
            "observation polling must never dispatch another removal"
        );
        assert_eq!(result, Ok(()));
        assert_eq!(fixture.ps_calls(), 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_rm_ignores_unrelated_and_recycled_names_when_proving_absence() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        // A successor reuses the selector's name under a new ID next to an unrelated container.
        let fixture = RemovalRaceRuntime::new(
            RM_ALREADY_IN_PROGRESS,
            "printf 'unrelated-id\\tother-container\\nsuccessor-id\\tstable-name\\n'\nexit 0",
        );

        let controller = fixture.controller(SUCCESSFUL_RUNTIME_TEST_TIMEOUT).await;
        let result = controller.reap_observed().await;

        assert_eq!(
            fixture.rm_calls(),
            [format!("rm -f {RACE_CONTAINER_ID}")],
            "only the captured ID may ever be removed, never the recycled name"
        );
        assert_eq!(result, Ok(()));
        assert_eq!(fixture.ps_calls(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_rm_with_a_still_present_object_never_greens_after_the_window() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        let fixture = RemovalRaceRuntime::new(
            RM_ALREADY_IN_PROGRESS,
            "printf '%s\\tstable-name\\n' \"$id\"\nexit 0",
        );

        let controller = fixture.controller(SUCCESSFUL_RUNTIME_TEST_TIMEOUT).await;
        let started = std::time::Instant::now();
        let result = controller.reap_observed().await;

        assert_eq!(fixture.rm_calls().len(), 1, "the removal must start once");
        assert_eq!(result, Err(ReapFailure::NonZeroExit));
        assert!(
            fixture.ps_calls() >= 2,
            "a present object must be observed repeatedly within the window, saw {}",
            fixture.ps_calls()
        );
        // The 2 s observation window, not the 5 s removal deadline, ends this observation.
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "observation must stop at its window, took {:?}",
            started.elapsed()
        );
        assert_eq!(
            fixture.rm_calls().len(),
            1,
            "observation polling must never dispatch another removal"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_rm_with_a_present_object_at_the_deadline_never_greens() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        let fixture = RemovalRaceRuntime::new(
            RM_ALREADY_IN_PROGRESS,
            "printf '%s\\tstable-name\\n' \"$id\"\nexit 0",
        );

        // The deadline, not the observation window, ends this observation.
        let started = std::time::Instant::now();
        let result = remove_container_id(
            &fixture.runtime(),
            RACE_CONTAINER_ID,
            Duration::from_millis(400),
        )
        .await;

        assert_eq!(fixture.rm_calls().len(), 1, "the removal must start once");
        assert_eq!(result, Err(ReapFailure::NonZeroExit));
        assert!(
            fixture.ps_calls() >= 1,
            "the object must have been observed"
        );
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "observation must stop at the shared deadline, took {:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_unusable_inventory_never_greens_and_keeps_its_typed_cause() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        // Neither exit status of `rm` is proof of absence, and an ambiguous observation must not be
        // collapsed into the removal's own failure code.
        for (label, rm_body, ps_body) in [
            ("failing inventory", RM_ALREADY_IN_PROGRESS, "exit 2"),
            (
                "malformed inventory",
                RM_ALREADY_IN_PROGRESS,
                "printf 'no-tab-separator\\n'\nexit 0",
            ),
            (
                "oversized inventory of valid rows",
                RM_ALREADY_IN_PROGRESS,
                // 3,500 whole 20-byte rows (70,000 bytes) that never name the captured ID, so only
                // the output limit refuses them. The bounded read stops 17 bytes into row 3,277,
                // after its tab, so the parser alone would call that truncated tail a valid absence.
                "yes \"$(printf 'other-id\\tother-name')\" | head -n 3500\nexit 0",
            ),
            ("failing inventory after a zero exit", RM_SUCCEEDS, "exit 2"),
        ] {
            let fixture = RemovalRaceRuntime::new(rm_body, ps_body);

            let result = remove_container_id(
                &fixture.runtime(),
                RACE_CONTAINER_ID,
                SUCCESSFUL_RUNTIME_TEST_TIMEOUT,
            )
            .await;

            assert_eq!(
                fixture.rm_calls().len(),
                1,
                "{label}: the removal must start once"
            );
            assert_eq!(result, Err(ReapFailure::IdentityUnavailable), "{label}");
            assert_eq!(
                fixture.ps_calls(),
                1,
                "{label}: an ambiguous observation fails closed without retrying"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_unspawnable_inventory_never_greens_and_keeps_its_typed_cause() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        // The runtime removes itself, so the confirming inventory cannot even spawn.
        for (label, rm_body) in [
            ("nonzero exit", "rm -f \"$0\"\nexit 1"),
            ("zero exit", "rm -f \"$0\"\nexit 0"),
        ] {
            let fixture = RemovalRaceRuntime::new(rm_body, "exit 0");

            let result = remove_container_id(
                &fixture.runtime(),
                RACE_CONTAINER_ID,
                SUCCESSFUL_RUNTIME_TEST_TIMEOUT,
            )
            .await;

            assert_eq!(
                fixture.rm_calls().len(),
                1,
                "{label}: the removal must start once"
            );
            assert_eq!(result, Err(ReapFailure::Spawn), "{label}");
            assert_eq!(fixture.ps_calls(), 0, "{label}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_rm_with_a_hung_inventory_times_out_at_the_shared_deadline() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        // `rm` consumes 1 s of the 1.5 s deadline; a fresh deadline for the inventory would end
        // near 2.5 s, so finishing well before that proves one shared absolute deadline.
        let fixture = RemovalRaceRuntime::new(
            "sleep 1\nexit 1",
            "while [ ! -e \"$dir/ps.release\" ]; do sleep 0.01; done\nexit 0",
        );

        let started = std::time::Instant::now();
        let result = remove_container_id(
            &fixture.runtime(),
            RACE_CONTAINER_ID,
            Duration::from_millis(1500),
        )
        .await;
        let elapsed = started.elapsed();

        assert_eq!(fixture.rm_calls().len(), 1, "the removal must start once");
        assert_eq!(fixture.ps_calls(), 1, "the inventory must have started");
        assert_eq!(result, Err(ReapFailure::Timeout));
        assert!(
            elapsed < Duration::from_millis(2200),
            "inventory work must share the original deadline, took {elapsed:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hung_rm_times_out_without_observing_the_inventory() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        let fixture = RemovalRaceRuntime::new(
            "while [ ! -e \"$dir/rm.release\" ]; do sleep 0.01; done\nexit 1",
            "exit 0",
        );

        let result = remove_container_id(
            &fixture.runtime(),
            RACE_CONTAINER_ID,
            Duration::from_millis(300),
        )
        .await;

        assert_eq!(fixture.rm_calls().len(), 1, "the removal must start once");
        assert_eq!(result, Err(ReapFailure::Timeout));
        assert_eq!(
            fixture.ps_calls(),
            0,
            "a timed-out removal is never observed"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exhausted_deadline_never_starts_an_absence_observation() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        // Even an inventory that would prove absence must not run once no deadline remains.
        let fixture = RemovalRaceRuntime::new(RM_ALREADY_IN_PROGRESS, "exit 0");

        let result = confirm_exact_id_absent_after_removal(
            &fixture.runtime(),
            RACE_CONTAINER_ID,
            tokio::time::Instant::now(),
        )
        .await;

        assert_eq!(result, Err(ReapFailure::Timeout));
        assert_eq!(
            fixture.ps_calls(),
            0,
            "no work may start after the deadline"
        );
        assert!(fixture.rm_calls().is_empty(), "observation never removes");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_rm_completes_only_after_the_exact_id_is_confirmed_absent() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        let fixture = RemovalRaceRuntime::new(RM_SUCCEEDS, "exit 0");

        let controller = fixture.controller(SUCCESSFUL_RUNTIME_TEST_TIMEOUT).await;
        let result = controller.reap_observed().await;

        assert_eq!(fixture.rm_calls(), [format!("rm -f {RACE_CONTAINER_ID}")]);
        assert_eq!(result, Ok(()));
        assert_eq!(
            fixture.ps_calls(),
            1,
            "a zero exit is confirmed by one inventory, never trusted as proof of absence"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_rm_with_a_still_present_object_never_greens() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        // A runtime wrapper that exits zero while leaving the captured ID present.
        let fixture =
            RemovalRaceRuntime::new(RM_SUCCEEDS, "printf '%s\\tstable-name\\n' \"$id\"\nexit 0");

        let started = std::time::Instant::now();
        let controller = fixture.controller(SUCCESSFUL_RUNTIME_TEST_TIMEOUT).await;
        let result = controller.reap_observed().await;

        assert_eq!(fixture.rm_calls().len(), 1, "the removal must start once");
        assert_eq!(result, Err(ReapFailure::NonZeroExit));
        assert!(
            fixture.ps_calls() >= 2,
            "the present object must be observed within the window, saw {}",
            fixture.ps_calls()
        );
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "observation must stop at its window, took {:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_inventory_that_reports_absence_after_the_window_never_greens() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        // The removal deadline is 5 s but the observation window is 2 s: an inventory that only
        // answers (absent) at 3 s is outside the window and must not settle cleanup.
        let fixture = RemovalRaceRuntime::new(RM_ALREADY_IN_PROGRESS, "sleep 3\nexit 0");

        let started = std::time::Instant::now();
        let result = remove_container_id(
            &fixture.runtime(),
            RACE_CONTAINER_ID,
            Duration::from_secs(5),
        )
        .await;

        assert_eq!(fixture.rm_calls().len(), 1, "the removal must start once");
        assert_eq!(fixture.ps_calls(), 1, "the inventory must have started");
        assert_eq!(result, Err(ReapFailure::Timeout));
        assert!(
            started.elapsed() < Duration::from_millis(2800),
            "the inventory child must be bounded by the window, took {:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_present_sighting_followed_by_an_interrupted_inventory_never_greens() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        // The first inventory sees the object; the second hangs until the 600 ms deadline cuts it
        // off. The object was never seen absent, so the removal did not achieve absence.
        let fixture = RemovalRaceRuntime::new(
            RM_ALREADY_IN_PROGRESS,
            "if [ \"$n\" -le 1 ]; then printf '%s\\tstable-name\\n' \"$id\"; exit 0; fi\nsleep 5\nexit 0",
        );

        let result = remove_container_id(
            &fixture.runtime(),
            RACE_CONTAINER_ID,
            Duration::from_millis(600),
        )
        .await;

        assert_eq!(fixture.rm_calls().len(), 1, "the removal must start once");
        assert_eq!(
            fixture.ps_calls(),
            2,
            "one sighting, then the interrupted inventory"
        );
        assert_eq!(result, Err(ReapFailure::NonZeroExit));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn absence_first_observed_after_the_bound_is_never_accepted() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        let fixture = RemovalRaceRuntime::new(RM_ALREADY_IN_PROGRESS, "exit 0");
        let runtime = fixture.runtime();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);

        // The inventory child answers (absent) long before the bound, but the only runtime thread
        // is then held past it, so the answer is first polled after the bound. `timeout_at` polls
        // its inner future first and would hand that stale answer back as success.
        let (result, ()) = tokio::join!(
            confirm_exact_id_absent_after_removal(&runtime, RACE_CONTAINER_ID, deadline),
            async { std::thread::sleep(Duration::from_millis(700)) },
        );

        assert_eq!(fixture.ps_calls(), 1, "the inventory must have run");
        assert_eq!(result, Err(ReapFailure::Timeout));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_malformed_inventory_never_settles_absence_after_either_exit_status() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        let mut wrong = Vec::new();
        for (exit, rm_body) in [("nonzero", RM_ALREADY_IN_PROGRESS), ("zero", RM_SUCCEEDS)] {
            for (label, row) in MALFORMED_INVENTORY_ROWS {
                let fixture =
                    RemovalRaceRuntime::new(rm_body, &format!("printf '%s\\n' '{row}'\nexit 0"));

                let result = remove_container_id(
                    &fixture.runtime(),
                    RACE_CONTAINER_ID,
                    SUCCESSFUL_RUNTIME_TEST_TIMEOUT,
                )
                .await;

                assert_eq!(
                    fixture.rm_calls().len(),
                    1,
                    "{label} after a {exit} exit: the removal must start once"
                );
                if result != Err(ReapFailure::IdentityUnavailable) || fixture.ps_calls() != 1 {
                    wrong.push(format!(
                        "{label} after a {exit} exit: {result:?} after {} inventories",
                        fixture.ps_calls()
                    ));
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "a malformed inventory must fail closed after one observation:\n{}",
            wrong.join("\n")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_matching_row_cannot_hide_a_malformed_row_from_the_removal() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        let mut wrong = Vec::new();
        for (label, ps_body) in [
            (
                "malformed row after the captured ID",
                "printf '%s\\tstable-name\\n' \"$id\"\nprintf 'other\\tname\\textra\\n'\nexit 0",
            ),
            (
                "malformed row before the captured ID",
                "printf 'other\\tname\\textra\\n'\nprintf '%s\\tstable-name\\n' \"$id\"\nexit 0",
            ),
            (
                "malformed row after the captured ID with no names",
                "printf '%s\\t\\n' \"$id\"\nprintf '\\tname\\n'\nexit 0",
            ),
        ] {
            let fixture = RemovalRaceRuntime::new(RM_ALREADY_IN_PROGRESS, ps_body);

            let result = remove_container_id(
                &fixture.runtime(),
                RACE_CONTAINER_ID,
                SUCCESSFUL_RUNTIME_TEST_TIMEOUT,
            )
            .await;

            assert_eq!(
                fixture.rm_calls().len(),
                1,
                "{label}: the removal must start once"
            );
            if result != Err(ReapFailure::IdentityUnavailable) || fixture.ps_calls() != 1 {
                wrong.push(format!(
                    "{label}: {result:?} after {} inventories",
                    fixture.ps_calls()
                ));
            }
        }
        assert!(
            wrong.is_empty(),
            "an inventory with any malformed row is unusable, whatever else it lists:\n{}",
            wrong.join("\n")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_present_sighting_followed_by_a_malformed_inventory_never_greens() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        // The first inventory sees the object; the second is malformed in the way that used to read
        // as "absent". A prior sighting must not turn that unusable answer into a completed removal.
        let fixture = RemovalRaceRuntime::new(
            RM_ALREADY_IN_PROGRESS,
            "if [ \"$n\" -le 1 ]; then printf '%s\\tstable-name\\n' \"$id\"; exit 0; fi\nprintf 'other\\tname\\textra\\n'\nexit 0",
        );

        let result = remove_container_id(
            &fixture.runtime(),
            RACE_CONTAINER_ID,
            SUCCESSFUL_RUNTIME_TEST_TIMEOUT,
        )
        .await;

        assert_eq!(fixture.rm_calls().len(), 1, "the removal must start once");
        assert_eq!(
            fixture.ps_calls(),
            2,
            "one sighting, then the malformed inventory"
        );
        assert_eq!(result, Err(ReapFailure::IdentityUnavailable));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_empty_names_field_still_observes_the_exact_id_as_present() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        // Mid-removal a runtime can list the captured ID with its names already released. That row
        // is well formed and the ID alone keeps the object present until it leaves the inventory.
        let fixture = RemovalRaceRuntime::new(
            RM_ALREADY_IN_PROGRESS,
            "if [ \"$n\" -le 2 ]; then printf '%s\\t\\n' \"$id\"; fi\nprintf 'other-id\\t\\n'\nexit 0",
        );

        let result = remove_container_id(
            &fixture.runtime(),
            RACE_CONTAINER_ID,
            SUCCESSFUL_RUNTIME_TEST_TIMEOUT,
        )
        .await;

        assert_eq!(
            fixture.rm_calls(),
            [format!("rm -f {RACE_CONTAINER_ID}")],
            "observation polling must never dispatch another removal"
        );
        assert_eq!(result, Ok(()));
        assert_eq!(
            fixture.ps_calls(),
            3,
            "two sightings by ID alone, then absence"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_empty_names_field_with_the_id_present_never_greens() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        let fixture =
            RemovalRaceRuntime::new(RM_ALREADY_IN_PROGRESS, "printf '%s\\t\\n' \"$id\"\nexit 0");

        let result = remove_container_id(
            &fixture.runtime(),
            RACE_CONTAINER_ID,
            SUCCESSFUL_RUNTIME_TEST_TIMEOUT,
        )
        .await;

        assert_eq!(fixture.rm_calls().len(), 1, "the removal must start once");
        assert_eq!(result, Err(ReapFailure::NonZeroExit));
        assert!(
            fixture.ps_calls() >= 2,
            "the present object must be observed within the window, saw {}",
            fixture.ps_calls()
        );
    }

    /// A production-attempt managed controller over a [`RemovalRaceRuntime`], with the journal
    /// exposed so a test can read the typed failure code that durable recovery would see.
    #[cfg(unix)]
    fn journaled_production_removal(
        fixture: &RemovalRaceRuntime,
    ) -> (
        ReapController,
        Arc<InMemoryResourceFlightJournal>,
        ResourceFlightIdV1,
    ) {
        let ownership = managed_ownership();
        let identity = ResourceIdentityV1::ManagedContainer {
            generation: format!("container-id:{RACE_CONTAINER_ID}"),
            runtime: fixture.runtime(),
            immutable_container_id: RACE_CONTAINER_ID.into(),
            ownership_labels_digest: ownership.digest().clone(),
        };
        let probe: ContainerIdentityProbeFn = {
            let labels = ownership.ordered().to_vec();
            Arc::new(move |_runtime, _selector| {
                let labels = labels.clone();
                Box::pin(async move {
                    Ok(ContainerRuntimeIdentityV1 {
                        immutable_container_id: RACE_CONTAINER_ID.into(),
                        ownership_labels: labels,
                    })
                })
            })
        };
        let subordinate: ContainerSubordinateCleanupFn = Arc::new(|| Box::pin(async { Ok(()) }));
        let journal = Arc::new(InMemoryResourceFlightJournal::new(512));
        let journal_port: Arc<dyn ResourceFlightJournal> = journal.clone();
        let (durable, flight_id) = durable_container_for_test(&identity, journal_port);
        let controller = ReapController::managed_durable_v3(
            identity,
            "stable-name",
            ownership,
            production_remove_attempt(SUCCESSFUL_RUNTIME_TEST_TIMEOUT),
            probe,
            subordinate,
            durable,
        )
        .unwrap();
        (controller, journal, flight_id)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_managed_flight_journals_the_typed_cause_of_an_unconfirmable_removal() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        for (label, rm_body, ps_body, expected) in [
            (
                "failing inventory",
                RM_ALREADY_IN_PROGRESS,
                "exit 2",
                ReapFailure::IdentityUnavailable,
            ),
            (
                "unspawnable inventory",
                "rm -f \"$0\"\nexit 1",
                "exit 0",
                ReapFailure::Spawn,
            ),
        ] {
            let fixture = RemovalRaceRuntime::new(rm_body, ps_body);
            let (controller, journal, flight_id) = journaled_production_removal(&fixture);

            let result = controller.reap_observed().await;

            assert_eq!(fixture.rm_calls().len(), 1, "{label}: one removal only");
            assert_eq!(result, Err(expected), "{label}");
            let rows = journal.records(&flight_id).unwrap();
            assert!(
                rows.iter().any(|row| matches!(
                    &row.event,
                    ResourceFlightJournalEventV1::ContainerRemovalObserved { observation }
                        if !observation.removed
                            && observation.failure_code.as_deref() == Some(expected.code())
                )),
                "{label}: the journaled removal must carry {}",
                expected.code()
            );
            assert!(
                rows.iter().any(|row| matches!(
                    &row.event,
                    ResourceFlightJournalEventV1::Settled { result }
                        if result.disposition == ResourceActionDispositionV1::Failed
                )),
                "{label}: the flight must settle failed"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_managed_flight_journals_a_confirmed_absent_removal_as_complete() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        let fixture = RemovalRaceRuntime::new(RM_ALREADY_IN_PROGRESS, "exit 0");
        let (controller, journal, flight_id) = journaled_production_removal(&fixture);

        let result = controller.reap_observed().await;

        assert_eq!(fixture.rm_calls().len(), 1, "one removal only");
        assert_eq!(result, Ok(()));
        let rows = journal.records(&flight_id).unwrap();
        assert!(rows.iter().any(|row| matches!(
            &row.event,
            ResourceFlightJournalEventV1::ContainerRemovalObserved { observation }
                if observation.removed && observation.failure_code.is_none()
        )));
        assert!(rows.iter().any(|row| matches!(
            &row.event,
            ResourceFlightJournalEventV1::Settled { result }
                if result.disposition == ResourceActionDispositionV1::Complete
        )));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_managed_flight_journals_a_malformed_inventory_as_unavailable_never_complete() {
        let _fixture_permit = production_runtime_fixture_permit().await;
        let mut wrong = Vec::new();
        for (label, rm_body, ps_body) in [
            (
                "extra-field inventory after a nonzero exit",
                RM_ALREADY_IN_PROGRESS,
                "printf 'other\\tname\\textra\\n'\nexit 0",
            ),
            (
                "extra-field inventory after a zero exit",
                RM_SUCCEEDS,
                "printf 'other\\tname\\textra\\n'\nexit 0",
            ),
            (
                "malformed row after the captured ID",
                RM_ALREADY_IN_PROGRESS,
                "printf '%s\\tstable-name\\n' \"$id\"\nprintf 'other\\tname\\textra\\n'\nexit 0",
            ),
            (
                "empty identity",
                RM_ALREADY_IN_PROGRESS,
                "printf '\\tname\\n'\nexit 0",
            ),
        ] {
            let fixture = RemovalRaceRuntime::new(rm_body, ps_body);
            let (controller, journal, flight_id) = journaled_production_removal(&fixture);

            let result = controller.reap_observed().await;

            assert_eq!(fixture.rm_calls().len(), 1, "{label}: one removal only");
            let rows = journal.records(&flight_id).unwrap();
            let journaled_unavailable = rows.iter().any(|row| {
                matches!(
                    &row.event,
                    ResourceFlightJournalEventV1::ContainerRemovalObserved { observation }
                        if !observation.removed
                            && observation.failure_code.as_deref()
                                == Some(ReapFailure::IdentityUnavailable.code())
                )
            });
            let journaled_removed = rows.iter().any(|row| {
                matches!(
                    &row.event,
                    ResourceFlightJournalEventV1::ContainerRemovalObserved { observation }
                        if observation.removed
                )
            });
            let settled = |disposition| {
                rows.iter().any(|row| {
                    matches!(
                        &row.event,
                        ResourceFlightJournalEventV1::Settled { result }
                            if result.disposition == disposition
                    )
                })
            };
            if result != Err(ReapFailure::IdentityUnavailable)
                || !journaled_unavailable
                || journaled_removed
                || !settled(ResourceActionDispositionV1::Failed)
                || settled(ResourceActionDispositionV1::Complete)
            {
                wrong.push(format!(
                    "{label}: returned {result:?}, journaled unavailable {journaled_unavailable}, \
                     journaled removed {journaled_removed}, settled failed {}, settled complete {}",
                    settled(ResourceActionDispositionV1::Failed),
                    settled(ResourceActionDispositionV1::Complete)
                ));
            }
        }
        assert!(
            wrong.is_empty(),
            "a malformed inventory must be returned and journaled as identity_unavailable:\n{}",
            wrong.join("\n")
        );
    }

    #[tokio::test]
    async fn joinable_reaper_runs_once_for_concurrent_waiters() {
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let attempt: ReapAttemptFn = {
            let calls = Arc::clone(&calls);
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            Arc::new(move |_runtime, _name| {
                let calls = Arc::clone(&calls);
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    entered.notify_one();
                    release.notified().await;
                    Ok(())
                })
            })
        };
        let controller = ReapController::new("docker", "a2a-rw-x", attempt);
        let first = {
            let controller = controller.clone();
            tokio::spawn(async move { controller.reap_observed().await })
        };
        entered.notified().await;
        let second = {
            let controller = controller.clone();
            tokio::spawn(async move { controller.reap_observed().await })
        };
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.notify_waiters();
        assert_eq!(first.await.unwrap(), Ok(()));
        assert_eq!(second.await.unwrap(), Ok(()));
        assert_eq!(controller.reap_observed().await, Ok(()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn joinable_reaper_returns_same_typed_failure_to_every_waiter() {
        for failure in [
            ReapFailure::Spawn,
            ReapFailure::Timeout,
            ReapFailure::NonZeroExit,
        ] {
            let attempt: ReapAttemptFn =
                Arc::new(move |_runtime, _name| Box::pin(async move { Err(failure) }));
            let controller = ReapController::new("docker", "a2a-rw-x", attempt);
            assert_eq!(controller.reap_observed().await, Err(failure));
            assert_eq!(controller.reap_observed().await, Err(failure));
            assert_eq!(controller.result(), Some(Err(failure)));
            assert_eq!(
                failure.code(),
                match failure {
                    ReapFailure::Spawn => "container.reap.spawn_failed",
                    ReapFailure::Timeout => "container.reap.timeout",
                    ReapFailure::NonZeroExit => "container.reap.nonzero_exit",
                    ReapFailure::WorkerPanicked => unreachable!(),
                    _ => unreachable!(),
                }
            );
        }
    }

    #[tokio::test]
    async fn synchronous_attempt_panic_settles_once_as_worker_panicked() {
        let calls = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_runtime, _name| {
                calls.fetch_add(1, Ordering::SeqCst);
                panic!("synchronous reaper panic")
            })
        };
        let controller = ReapController::new("docker", "a2a-rw-sync-panic", attempt);

        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::WorkerPanicked)
        );
        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::WorkerPanicked)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn asynchronous_attempt_panic_settles_once_as_worker_panicked() {
        let calls = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_runtime, _name| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    panic!("asynchronous reaper panic")
                })
            })
        };
        let controller = ReapController::new("docker", "a2a-rw-async-panic", attempt);

        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::WorkerPanicked)
        );
        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::WorkerPanicked)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn production_timeout_kills_child_before_delayed_side_effect() {
        use std::os::unix::fs::PermissionsExt;

        let _fixture_permit = production_runtime_fixture_permit().await;
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("hung-runtime");
        let marker = temp.path().join("late-side-effect");
        let release = temp.path().join("late-side-effect.release");
        std::fs::write(
            &runtime,
            "#!/bin/sh\n[ \"$#\" -eq 0 ] && exit 0\nwhile [ ! -e \"$5.release\" ]; do sleep 0.01; done\nprintf reached > \"$5\"\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&runtime, permissions).unwrap();
        // The no-argument branch is the side-effect-free warm-up. A refused spawn would surface as
        // `Spawn`, not the `Timeout` this control requires.
        warm_up_runtime_through_a_held_writer(&runtime);

        let controller = ReapController::production_with_timeout(
            runtime.to_string_lossy(),
            marker.to_string_lossy(),
            Duration::from_millis(20),
        );
        assert_eq!(controller.reap_observed().await, Err(ReapFailure::Timeout));
        // Give the kill-on-drop signal time to settle before allowing a live
        // runtime to continue. Unlike a fixed child delay, this gate cannot
        // elapse during a heavily loaded synchronous spawn-to-timeout gap.
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::write(release, b"release").unwrap();
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(
            !marker.exists(),
            "kill_on_drop must stop the timed-out runtime before its delayed side effect"
        );
    }

    #[tokio::test]
    async fn detached_reap_starts_the_same_joinable_attempt() {
        let calls = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_runtime, _name| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let controller = ReapController::new("docker", "a2a-rw-x", attempt);
        controller.reap_detached();
        tokio::time::timeout(Duration::from_secs(1), controller.reap_observed())
            .await
            .expect("detached attempt settles")
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn legacy_reap_fn_adapter_is_joinable_and_exactly_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reap_fn: ReapFn = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_runtime, _name| {
                calls.fetch_add(1, Ordering::SeqCst);
            })
        };
        let controller = ReapController::from_legacy("docker", "a2a-rw-x", reap_fn);
        controller.reap_observed().await.unwrap();
        controller.reap_detached();
        controller.reap_observed().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    fn managed_ownership() -> CanonicalContainerOwnershipV1 {
        crate::run_identity::ContainerLabels {
            role: "rw".into(),
            kind: "warm".into(),
            agent: "impl".into(),
            owner: "owner".into(),
            run_id: "run".into(),
            host: "host".into(),
            lease: "/lease".into(),
            repo: None,
            cwd: None,
            start: "1".into(),
        }
        .canonical_ownership()
    }

    fn managed_identity(
        ownership: &CanonicalContainerOwnershipV1,
        immutable_container_id: &str,
    ) -> ResourceIdentityV1 {
        ResourceIdentityV1::ManagedContainer {
            generation: format!("container-id:{immutable_container_id}"),
            runtime: "docker".into(),
            immutable_container_id: immutable_container_id.into(),
            ownership_labels_digest: ownership.digest().clone(),
        }
    }

    fn durable_fault_controller(
        fault: ContainerJournalFault,
    ) -> (
        ReapController,
        Arc<FaultContainerJournal>,
        ResourceFlightIdV1,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
    ) {
        let ownership = managed_ownership();
        let immutable_id = "sha256:fault-test";
        let identity = managed_identity(&ownership, immutable_id);
        let removal_calls = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let removal_calls = Arc::clone(&removal_calls);
            Arc::new(move |_runtime, target| {
                let removal_calls = Arc::clone(&removal_calls);
                Box::pin(async move {
                    assert_eq!(target, "sha256:fault-test");
                    removal_calls.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    Ok(())
                })
            })
        };
        let probe: ContainerIdentityProbeFn = {
            let labels = ownership.ordered().to_vec();
            Arc::new(move |_runtime, selector| {
                let labels = labels.clone();
                Box::pin(async move {
                    assert_eq!(selector, "stable-name");
                    Ok(ContainerRuntimeIdentityV1 {
                        immutable_container_id: "sha256:fault-test".into(),
                        ownership_labels: labels,
                    })
                })
            })
        };
        let subordinate_calls = Arc::new(AtomicUsize::new(0));
        let subordinate: ContainerSubordinateCleanupFn = {
            let subordinate_calls = Arc::clone(&subordinate_calls);
            Arc::new(move || {
                let subordinate_calls = Arc::clone(&subordinate_calls);
                Box::pin(async move {
                    subordinate_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let journal = Arc::new(FaultContainerJournal::new(fault));
        let journal_port: Arc<dyn ResourceFlightJournal> = journal.clone();
        let (durable, flight_id) = durable_container_for_test(&identity, journal_port);
        let controller = ReapController::managed_durable_v3(
            identity,
            "stable-name",
            ownership,
            attempt,
            probe,
            subordinate,
            durable,
        )
        .unwrap();
        (
            controller,
            journal,
            flight_id,
            removal_calls,
            subordinate_calls,
        )
    }

    #[tokio::test]
    async fn managed_flight_removes_only_the_captured_id_and_composes_once() {
        let ownership = managed_ownership();
        let immutable_id = "sha256:captured";
        let removals = Arc::new(StdMutex::new(Vec::new()));
        let attempt: ReapAttemptFn = {
            let removals = Arc::clone(&removals);
            Arc::new(move |runtime, target| {
                let removals = Arc::clone(&removals);
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    removals.lock().unwrap().push((runtime, target));
                    Ok(())
                })
            })
        };
        let probe: ContainerIdentityProbeFn = {
            let labels = ownership.ordered().to_vec();
            Arc::new(move |_runtime, selector| {
                let labels = labels.clone();
                Box::pin(async move {
                    assert_eq!(selector, "stable-name");
                    Ok(ContainerRuntimeIdentityV1 {
                        immutable_container_id: "sha256:captured".into(),
                        ownership_labels: labels,
                    })
                })
            })
        };
        let subordinate_calls = Arc::new(AtomicUsize::new(0));
        let subordinate_token = Arc::new(());
        let subordinate_weak = Arc::downgrade(&subordinate_token);
        let subordinate: ContainerSubordinateCleanupFn = {
            let subordinate_calls = Arc::clone(&subordinate_calls);
            let subordinate_token = Arc::clone(&subordinate_token);
            Arc::new(move || {
                let subordinate_calls = Arc::clone(&subordinate_calls);
                let subordinate_token = Arc::clone(&subordinate_token);
                Box::pin(async move {
                    let _subordinate_token = subordinate_token;
                    subordinate_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let controller = ReapController::managed_legacy_v2(
            managed_identity(&ownership, immutable_id),
            "stable-name",
            ownership,
            attempt,
            probe,
            subordinate,
        )
        .unwrap();
        drop(subordinate_token);
        assert!(subordinate_weak.upgrade().is_some());
        controller
            .attach_owner(
                ResourceFlightOwnerV1::new(NodeId::parse("session-owner").unwrap(), "session-1")
                    .unwrap(),
            )
            .unwrap();

        controller.reap_detached();
        let (first, second) = tokio::join!(controller.reap_observed(), controller.reap_observed());
        assert_eq!(first, Ok(()));
        assert_eq!(second, Ok(()));
        assert_eq!(controller.reap_observed().await, Ok(()));
        assert_eq!(subordinate_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            *removals.lock().unwrap(),
            vec![("docker".into(), immutable_id.into())]
        );
        assert!(
            subordinate_weak.upgrade().is_none(),
            "settlement must release the one-shot subordinate closure"
        );
    }

    #[tokio::test]
    async fn protected_subordinate_failure_cannot_collapse_to_complete() {
        let ownership = managed_ownership();
        let immutable_id = "sha256:protected";
        let identity = managed_identity(&ownership, immutable_id);
        let actions = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let actions = Arc::clone(&actions);
            Arc::new(move |_runtime, target| {
                let actions = Arc::clone(&actions);
                Box::pin(async move {
                    assert_eq!(target, "sha256:protected");
                    actions.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let probe: ContainerIdentityProbeFn = {
            let labels = ownership.ordered().to_vec();
            Arc::new(move |_runtime, _selector| {
                let labels = labels.clone();
                Box::pin(async move {
                    Ok(ContainerRuntimeIdentityV1 {
                        immutable_container_id: "sha256:protected".into(),
                        ownership_labels: labels,
                    })
                })
            })
        };
        let subordinate: ContainerSubordinateCleanupFn = {
            let actions = Arc::clone(&actions);
            Arc::new(move || {
                let actions = Arc::clone(&actions);
                Box::pin(async move {
                    actions.fetch_add(1, Ordering::SeqCst);
                    Err(())
                })
            })
        };
        let journal_root = tempfile::tempdir().unwrap();
        let journal = Arc::new(FileResourceFlightJournal::open(journal_root.path(), 64).unwrap());
        let route = crate::process::DurableProcessFlightAttemptV3::new(
            AttemptId::mint().unwrap(),
            Arc::clone(&journal),
        );
        let generation = match &identity {
            ResourceIdentityV1::ManagedContainer { generation, .. } => generation.clone(),
            _ => unreachable!(),
        };
        let durable = route
            .bind_container_generation(
                generation,
                ResourceFlightOwnerV1::new(
                    NodeId::parse("container-spawn").unwrap(),
                    "sha256:protected",
                )
                .unwrap(),
            )
            .unwrap();
        let controller = ReapController::managed_durable_v3(
            identity,
            "stable-name",
            ownership,
            attempt,
            probe,
            subordinate,
            durable,
        )
        .unwrap();
        assert!(controller.is_protected_v3());
        let flight_id = controller
            .managed
            .as_ref()
            .unwrap()
            .flight
            .flight_id()
            .clone();
        assert!(journal.records(&flight_id).unwrap().iter().any(|row| {
            matches!(
                &row.event,
                ResourceFlightJournalEventV1::ContainerIdentityCaptured {
                    identity: ResourceIdentityV1::ManagedContainer {
                        immutable_container_id,
                        ..
                    }
                } if immutable_container_id == "sha256:protected"
            )
        }));
        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::SubordinateCleanup)
        );
        assert_eq!(actions.load(Ordering::SeqCst), 2);
        let rows = journal.records(&flight_id).unwrap();
        assert!(rows.iter().any(|row| matches!(
            &row.event,
            ResourceFlightJournalEventV1::ContainerRemovalObserved { observation }
                if observation.removed && observation.failure_code.is_none()
        )));
        assert!(rows.iter().any(|row| matches!(
            &row.event,
            ResourceFlightJournalEventV1::Settled { result }
                if result.disposition == ResourceActionDispositionV1::Failed
        )));
    }

    #[tokio::test]
    async fn predispatch_failure_settles_failed_and_runs_subordinate_without_hanging() {
        let (controller, journal, flight_id, removal_calls, subordinate_calls) =
            durable_fault_controller(ContainerJournalFault::Dispatch);

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), controller.reap_observed())
                .await
                .expect("predispatch refusal must terminalize"),
            Err(ReapFailure::FlightRefused)
        );
        assert_eq!(removal_calls.load(Ordering::SeqCst), 0);
        assert_eq!(subordinate_calls.load(Ordering::SeqCst), 1);
        assert!(journal
            .records(&flight_id)
            .unwrap()
            .iter()
            .any(|row| matches!(
                &row.event,
                ResourceFlightJournalEventV1::Settled { result }
                    if result.disposition == ResourceActionDispositionV1::Failed
            )));
    }

    #[tokio::test]
    async fn successful_removal_is_not_reclassified_when_its_observation_write_fails() {
        let (controller, journal, flight_id, removal_calls, subordinate_calls) =
            durable_fault_controller(ContainerJournalFault::RemovalObservation);

        assert_eq!(controller.reap_observed().await, Ok(()));
        assert_eq!(controller.reap_observed().await, Ok(()));
        assert_eq!(removal_calls.load(Ordering::SeqCst), 1);
        assert_eq!(subordinate_calls.load(Ordering::SeqCst), 1);
        let rows = journal.records(&flight_id).unwrap();
        assert!(!rows.iter().any(|row| matches!(
            &row.event,
            ResourceFlightJournalEventV1::ContainerRemovalObserved { .. }
        )));
        assert!(rows.iter().any(|row| matches!(
            &row.event,
            ResourceFlightJournalEventV1::Settled { result }
                if result.disposition == ResourceActionDispositionV1::Complete
                    && result.duration_ms > 0
        )));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recovery_terminal_winner_is_returned_by_driver_and_joiner() {
        let (controller, journal, flight_id, removal_calls, subordinate_calls) =
            durable_fault_controller(ContainerJournalFault::BlockCompleteTerminal);
        let driver = {
            let controller = controller.clone();
            tokio::spawn(async move { controller.reap_observed().await })
        };

        journal.driver_at_terminal.as_ref().unwrap().wait();
        let publisher = RecordingPublisher::default();
        let recovered = RetainedResourceFlight::recover_dead_journaled_intent_as_unknown(
            journal.as_ref(),
            &flight_id,
            17,
            &publisher,
        )
        .unwrap()
        .expect("recovery wins the terminal CAS");
        journal.recovery_done.as_ref().unwrap().wait();

        assert_eq!(recovered.disposition, ResourceActionDispositionV1::Unknown);
        assert_eq!(driver.await.unwrap(), Err(ReapFailure::IdentityUnavailable));
        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::IdentityUnavailable)
        );
        assert_eq!(removal_calls.load(Ordering::SeqCst), 1);
        assert_eq!(subordinate_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            publisher
                .0
                .lock()
                .unwrap()
                .iter()
                .map(|aggregation| aggregation.result.disposition.clone())
                .collect::<Vec<_>>(),
            vec![ResourceActionDispositionV1::Unknown]
        );
        let terminal = journal
            .records(&flight_id)
            .unwrap()
            .into_iter()
            .filter_map(|row| match row.event {
                ResourceFlightJournalEventV1::Settled { result } => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(terminal, vec![recovered]);
    }

    #[tokio::test]
    async fn probe_timeout_runs_subordinate_once_and_releases_captured_inner() {
        let ownership = managed_ownership();
        let removals = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let removals = Arc::clone(&removals);
            Arc::new(move |_runtime, _target| {
                let removals = Arc::clone(&removals);
                Box::pin(async move {
                    removals.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let probe: ContainerIdentityProbeFn =
            Arc::new(move |_runtime, _selector| Box::pin(async move { Err(ReapFailure::Timeout) }));
        let subordinate_calls = Arc::new(AtomicUsize::new(0));
        let inner = Arc::new(());
        let inner_weak = Arc::downgrade(&inner);
        let subordinate: ContainerSubordinateCleanupFn = {
            let subordinate_calls = Arc::clone(&subordinate_calls);
            let inner = Arc::clone(&inner);
            Arc::new(move || {
                let subordinate_calls = Arc::clone(&subordinate_calls);
                let inner = Arc::clone(&inner);
                Box::pin(async move {
                    let _inner = inner;
                    subordinate_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let controller = ReapController::managed_legacy_v2(
            managed_identity(&ownership, "sha256:original"),
            "recycled-name",
            ownership,
            attempt,
            probe,
            subordinate,
        )
        .unwrap();
        drop(inner);

        controller.reap_detached();
        assert_eq!(controller.reap_observed().await, Err(ReapFailure::Timeout));
        assert_eq!(controller.reap_observed().await, Err(ReapFailure::Timeout));
        assert_eq!(removals.load(Ordering::SeqCst), 0);
        assert_eq!(subordinate_calls.load(Ordering::SeqCst), 1);
        assert!(inner_weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn extra_noncanonical_a2a_label_is_tolerated_and_both_actions_run() {
        let ownership = managed_ownership();
        let identity = managed_identity(&ownership, "sha256:captured");
        let journal = Arc::new(InMemoryResourceFlightJournal::new(64));
        let actions = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let actions = Arc::clone(&actions);
            Arc::new(move |_runtime, _target| {
                let actions = Arc::clone(&actions);
                Box::pin(async move {
                    actions.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let probe: ContainerIdentityProbeFn = {
            let mut labels = ownership.ordered().to_vec();
            labels.push(("a2a.future".into(), "unexpected".into()));
            Arc::new(move |_runtime, _selector| {
                let labels = labels.clone();
                Box::pin(async move {
                    Ok(ContainerRuntimeIdentityV1 {
                        immutable_container_id: "sha256:captured".into(),
                        ownership_labels: labels,
                    })
                })
            })
        };
        let subordinate: ContainerSubordinateCleanupFn = {
            let actions = Arc::clone(&actions);
            Arc::new(move || {
                let actions = Arc::clone(&actions);
                Box::pin(async move {
                    actions.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let journal_port: Arc<dyn ResourceFlightJournal> = journal.clone();
        let (durable, flight_id) = durable_container_for_test(&identity, journal_port);
        let controller = ReapController::managed_durable_v3(
            identity,
            "stable-name",
            ownership,
            attempt,
            probe,
            subordinate,
            durable,
        )
        .unwrap();
        assert_eq!(controller.reap_observed().await, Ok(()));
        assert_eq!(actions.load(Ordering::SeqCst), 2);
        let labels = journal
            .records(&flight_id)
            .unwrap()
            .into_iter()
            .find_map(|row| match row.event {
                ResourceFlightJournalEventV1::ContainerRemovalObserved { observation } => {
                    Some(observation.observed_noncanonical_a2a_labels)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(labels, vec![("a2a.future".into(), "unexpected".into())]);
    }

    // ---- plan_recovery (pure crash-recovery planner) ---------------------------------------------------
    use crate::liveness::LeaseProbe;

    /// Map `lease_path -> Some(true)=free/dead | Some(false)=held/alive | None=absent`.
    struct MapProbe(std::collections::HashMap<String, Option<bool>>);
    impl LeaseProbe for MapProbe {
        fn try_state(&self, lease_path: &str) -> Option<bool> {
            self.0.get(lease_path).copied().flatten()
        }
    }
    fn rec(id: &str, host: &str, lease: &str) -> (String, String, String) {
        (id.to_string(), host.to_string(), lease.to_string())
    }

    #[test]
    fn plan_recovery_reaps_only_dead_same_host() {
        let probe = MapProbe(std::collections::HashMap::from([
            ("/l/dead.lock".to_string(), Some(true)),   // free ⇒ dead
            ("/l/alive.lock".to_string(), Some(false)), // held ⇒ alive
            ("/l/gone.lock".to_string(), None),         // absent ⇒ unknown
        ]));
        let records = vec![
            rec("c_dead", "h1", "/l/dead.lock"),
            rec("c_alive", "h1", "/l/alive.lock"),
            rec("c_absent", "h1", "/l/gone.lock"),
            rec("c_otherhost", "h2", "/l/dead.lock"), // free lock but DIFFERENT host ⇒ spared
        ];
        let (reap, leases) = plan_recovery(&records, "h1", &probe);
        assert_eq!(reap, vec!["c_dead".to_string()]);
        assert_eq!(leases, vec!["/l/dead.lock".to_string()]);
    }

    #[test]
    fn plan_recovery_shared_lease_across_owners_reaps_all_and_dedups_lease() {
        // The live-gate keystone: a crashed run's :rw + :ro containers (distinct ids, DIFFERENT owners) share
        // ONE free lease. Classified as a single batch BEFORE any deletion ⇒ ALL reaped, the lease returned
        // EXACTLY ONCE for the caller to delete after every owner is swept (no mid-pass blinding).
        let probe = MapProbe(std::collections::HashMap::from([(
            "/l/run.lock".to_string(),
            Some(true),
        )]));
        let records = vec![
            rec("c_rw", "h1", "/l/run.lock"),
            rec("c_ro_codex", "h1", "/l/run.lock"),
            rec("c_ro_claude", "h1", "/l/run.lock"),
        ];
        let (reap, leases) = plan_recovery(&records, "h1", &probe);
        assert_eq!(
            reap,
            vec![
                "c_rw".to_string(),
                "c_ro_codex".to_string(),
                "c_ro_claude".to_string()
            ]
        );
        assert_eq!(leases, vec!["/l/run.lock".to_string()]); // deduped to one
    }

    #[test]
    fn plan_recovery_blank_lease_label_is_spared() {
        // A blank a2a.lease label probes to None ⇒ classify spares (Unknown): never reaped, no dead lease.
        let probe = MapProbe(std::collections::HashMap::new());
        let records = vec![rec("c", "h1", "")];
        let (reap, leases) = plan_recovery(&records, "h1", &probe);
        assert!(reap.is_empty());
        assert!(leases.is_empty());
    }
}
