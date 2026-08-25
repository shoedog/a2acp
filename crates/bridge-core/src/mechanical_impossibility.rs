//! Constructive evidence that a pending producer can no longer yield a terminal result.
//!
//! The proof is deliberately sealed. Workflow code can submit authoritative producer and route
//! observations, but it cannot construct the proof itself. Ambiguous observations and
//! non-constructive liveness signals return `None`.

#![allow(dead_code)]

use crate::resource_flight::ResourceIdentityV1;
use crate::retained_resource_flight::{ContainerRemovalObservationV1, ProcessSignalObservationV1};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MechanicalImpossibilityKindV1 {
    RetainedChildExited,
    NamedContainerGenerationAbsent,
    AllProducerAndFinalRoutesClosed,
}

/// A sealed witness that a terminal result is mechanically impossible.
///
/// There is intentionally no default, boolean conversion, public field, or unchecked constructor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MechanicalImpossibilityProofV1 {
    kind: MechanicalImpossibilityKindV1,
}

impl MechanicalImpossibilityProofV1 {
    #[must_use]
    pub const fn kind(&self) -> MechanicalImpossibilityKindV1 {
        self.kind
    }
}

pub enum ProducerResultObservationV1 {
    PendingSoleProducer,
    PendingMultipleProducers,
    TerminalResultObserved,
    Unknown,
}

pub enum ContainerSpawnSettlementV1 {
    Settled,
    Pending,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteStateV1 {
    Open,
    IrreversiblyClosed,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalResultObservationV1 {
    Absent,
    Present,
    Unknown,
}

pub struct ProducerFinalRouteObservationV1 {
    pub producer_routes: Vec<RouteStateV1>,
    pub final_routes: Vec<RouteStateV1>,
    pub terminal_result: TerminalResultObservationV1,
}

/// The complete observation vocabulary accepted by the proof classifier.
///
/// The last six variants are explicit non-proofs. Their presence prevents silence or incidental
/// filesystem/process metadata from being reinterpreted as constructive evidence by a caller.
pub enum MechanicalImpossibilityObservationV1<'a> {
    RetainedChild {
        identity: &'a ResourceIdentityV1,
        signal: &'a ProcessSignalObservationV1,
        producer_result: ProducerResultObservationV1,
    },
    NamedContainerGeneration {
        identity: &'a ResourceIdentityV1,
        removal: &'a ContainerRemovalObservationV1,
        spawn_settlement: ContainerSpawnSettlementV1,
    },
    ProducerAndFinalRoutes(&'a ProducerFinalRouteObservationV1),
    UnknownChildState,
    NoOutput,
    ElapsedSilence,
    FileMtime,
    ProcessAge,
    ProviderSlowness,
}

pub fn prove_mechanical_impossibility_v1(
    observation: MechanicalImpossibilityObservationV1<'_>,
) -> Option<MechanicalImpossibilityProofV1> {
    let kind = match observation {
        MechanicalImpossibilityObservationV1::RetainedChild {
            identity,
            signal,
            producer_result: ProducerResultObservationV1::PendingSoleProducer,
        } if retained_child_exit_is_unambiguous(identity, signal) => {
            MechanicalImpossibilityKindV1::RetainedChildExited
        }
        MechanicalImpossibilityObservationV1::NamedContainerGeneration {
            identity,
            removal,
            spawn_settlement: ContainerSpawnSettlementV1::Settled,
        } if container_absence_is_unambiguous(identity, removal) => {
            MechanicalImpossibilityKindV1::NamedContainerGenerationAbsent
        }
        MechanicalImpossibilityObservationV1::ProducerAndFinalRoutes(routes)
            if all_routes_are_irreversibly_closed(routes) =>
        {
            MechanicalImpossibilityKindV1::AllProducerAndFinalRoutesClosed
        }
        _ => return None,
    };
    Some(MechanicalImpossibilityProofV1 { kind })
}

fn retained_child_exit_is_unambiguous(
    identity: &ResourceIdentityV1,
    signal: &ProcessSignalObservationV1,
) -> bool {
    let ResourceIdentityV1::AcpProcess {
        pid,
        immutable_start,
        ..
    } = identity
    else {
        return false;
    };
    *pid != 0
        && *pid == immutable_start.pid
        && signal.pid == *pid
        && signal.expected_start_time_ticks == immutable_start.start_time_ticks
        && signal.signal == 0
        && signal.return_code == -1
        && signal.errno == Some(libc::ESRCH)
}

fn container_absence_is_unambiguous(
    identity: &ResourceIdentityV1,
    removal: &ContainerRemovalObservationV1,
) -> bool {
    let ResourceIdentityV1::ManagedContainer {
        generation,
        runtime,
        immutable_container_id,
        ..
    } = identity
    else {
        return false;
    };
    !generation.is_empty()
        && !runtime.is_empty()
        && !immutable_container_id.is_empty()
        && removal.immutable_container_id == *immutable_container_id
        && removal.observed_noncanonical_a2a_labels.is_empty()
        && removal.removed
        && removal.failure_code.is_none()
}

fn all_routes_are_irreversibly_closed(routes: &ProducerFinalRouteObservationV1) -> bool {
    !routes.producer_routes.is_empty()
        && !routes.final_routes.is_empty()
        && routes
            .producer_routes
            .iter()
            .chain(&routes.final_routes)
            .all(|route| *route == RouteStateV1::IrreversiblyClosed)
        && routes.terminal_result == TerminalResultObservationV1::Absent
}

#[cfg(test)]
#[path = "mechanical_impossibility_tests.rs"]
mod tests;
