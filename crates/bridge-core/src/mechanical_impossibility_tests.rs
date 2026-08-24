use super::MechanicalImpossibilityObservationV1 as Observation;
use super::*;
use crate::execution_policy::Sha256HexV1;
use crate::resource_flight::ProcessStartIdentityV1;

fn acp_process() -> ResourceIdentityV1 {
    ResourceIdentityV1::AcpProcess {
        generation: "generation-1".into(),
        spawn_nonce_sha256: Sha256HexV1::digest(b"spawn"),
        pid: 41,
        pgid: Some(41),
        immutable_start: ProcessStartIdentityV1 {
            pid: 41,
            start_time_ticks: 73,
            executable_sha256: Some(Sha256HexV1::digest(b"executable")),
        },
    }
}

fn exited_observation() -> ProcessSignalObservationV1 {
    ProcessSignalObservationV1 {
        pid: 41,
        expected_start_time_ticks: 73,
        signal: 0,
        return_code: -1,
        errno: Some(libc::ESRCH),
    }
}

fn managed_container() -> ResourceIdentityV1 {
    ResourceIdentityV1::ManagedContainer {
        generation: "generation-2".into(),
        runtime: "docker".into(),
        immutable_container_id: "container-123".into(),
        ownership_labels_digest: Sha256HexV1::digest(b"labels"),
    }
}

fn removed_observation() -> ContainerRemovalObservationV1 {
    ContainerRemovalObservationV1 {
        immutable_container_id: "container-123".into(),
        observed_noncanonical_a2a_labels: Vec::new(),
        removed: true,
        failure_code: None,
    }
}

fn prove(observation: Observation<'_>) -> bool {
    prove_mechanical_impossibility_v1(observation).is_some()
}

fn route_observation(
    terminal_result: TerminalResultObservationV1,
) -> ProducerFinalRouteObservationV1 {
    ProducerFinalRouteObservationV1 {
        producer_routes: vec![RouteStateV1::IrreversiblyClosed],
        final_routes: vec![RouteStateV1::IrreversiblyClosed],
        terminal_result,
    }
}

#[test]
fn retained_child_exit_with_pending_sole_result_yields_proof() {
    let identity = acp_process();
    let signal = exited_observation();
    let proof = prove_mechanical_impossibility_v1(Observation::RetainedChild {
        identity: &identity,
        signal: &signal,
        producer_result: ProducerResultObservationV1::PendingSoleProducer,
    })
    .unwrap();
    assert_eq!(
        proof.kind(),
        MechanicalImpossibilityKindV1::RetainedChildExited
    );
}

#[test]
fn settled_named_container_generation_absence_yields_proof() {
    let identity = managed_container();
    let removal = removed_observation();
    let proof = prove_mechanical_impossibility_v1(Observation::NamedContainerGeneration {
        identity: &identity,
        removal: &removal,
        spawn_settlement: ContainerSpawnSettlementV1::Settled,
    })
    .unwrap();
    assert_eq!(
        proof.kind(),
        MechanicalImpossibilityKindV1::NamedContainerGenerationAbsent
    );
}

#[test]
fn all_producer_and_final_routes_closed_yields_proof() {
    let routes = route_observation(TerminalResultObservationV1::Absent);
    let proof =
        prove_mechanical_impossibility_v1(Observation::ProducerAndFinalRoutes(&routes)).unwrap();
    assert_eq!(
        proof.kind(),
        MechanicalImpossibilityKindV1::AllProducerAndFinalRoutesClosed
    );
}

#[test]
fn unknown_child_state_is_not_proof() {
    assert!(!prove(Observation::UnknownChildState));
}

#[test]
fn no_output_is_not_proof() {
    assert!(!prove(Observation::NoOutput));
}

#[test]
fn elapsed_silence_is_not_proof() {
    assert!(!prove(Observation::ElapsedSilence));
}

#[test]
fn file_mtime_is_not_proof() {
    assert!(!prove(Observation::FileMtime));
}

#[test]
fn process_age_is_not_proof() {
    assert!(!prove(Observation::ProcessAge));
}

#[test]
fn provider_slowness_is_not_proof() {
    assert!(!prove(Observation::ProviderSlowness));
}

#[test]
fn pid_reuse_or_undetermined_errno_is_not_child_exit_proof() {
    let identity = acp_process();
    for signal in [
        ProcessSignalObservationV1 {
            expected_start_time_ticks: 74,
            ..exited_observation()
        },
        ProcessSignalObservationV1 {
            errno: Some(libc::EIO),
            ..exited_observation()
        },
        ProcessSignalObservationV1 {
            errno: None,
            ..exited_observation()
        },
    ] {
        assert!(!prove(Observation::RetainedChild {
            identity: &identity,
            signal: &signal,
            producer_result: ProducerResultObservationV1::PendingSoleProducer,
        }));
    }
}

#[test]
fn ambiguous_container_removal_is_not_absence_proof() {
    let identity = managed_container();
    for removal in [
        ContainerRemovalObservationV1 {
            removed: false,
            ..removed_observation()
        },
        ContainerRemovalObservationV1 {
            failure_code: Some("container.reap.timeout".into()),
            ..removed_observation()
        },
        ContainerRemovalObservationV1 {
            observed_noncanonical_a2a_labels: vec![("a2a.owner".into(), "other".into())],
            ..removed_observation()
        },
    ] {
        assert!(!prove(Observation::NamedContainerGeneration {
            identity: &identity,
            removal: &removal,
            spawn_settlement: ContainerSpawnSettlementV1::Settled,
        }));
    }
}

#[test]
fn mismatched_container_id_is_not_absence_proof() {
    let identity = managed_container();
    let removal = ContainerRemovalObservationV1 {
        immutable_container_id: "different-container".into(),
        ..removed_observation()
    };
    assert!(!prove(Observation::NamedContainerGeneration {
        identity: &identity,
        removal: &removal,
        spawn_settlement: ContainerSpawnSettlementV1::Settled,
    }));
}

#[test]
fn one_open_route_prevents_all_routes_closed_proof() {
    let mut routes = route_observation(TerminalResultObservationV1::Absent);
    routes.producer_routes[0] = RouteStateV1::Open;
    assert!(!prove(Observation::ProducerAndFinalRoutes(&routes)));
    routes.producer_routes[0] = RouteStateV1::IrreversiblyClosed;
    routes.final_routes[0] = RouteStateV1::Open;
    assert!(!prove(Observation::ProducerAndFinalRoutes(&routes)));
}

#[test]
fn present_or_unknown_terminal_result_prevents_all_routes_closed_proof() {
    for terminal_result in [
        TerminalResultObservationV1::Present,
        TerminalResultObservationV1::Unknown,
    ] {
        let routes = route_observation(terminal_result);
        assert!(!prove(Observation::ProducerAndFinalRoutes(&routes)));
    }
}
