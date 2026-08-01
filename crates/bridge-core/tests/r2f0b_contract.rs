use bridge_core::terminal_evidence::{
    resolve_terminal, AcpChildLiveness, EvidenceCapability, EvidenceCompleteness, FinalPresence,
    ProducerTerminal, PromptRpcObservation, TerminalObservation,
};

#[test]
fn downstream_r2f0b_core_api_surface_is_exported() {
    use bridge_core::attempt_activity::{ActivityReason, AttemptActivity, AttemptPhase};
    use bridge_core::ports::AgentBackend;
    use bridge_core::terminal_evidence::TerminalEvidenceSink;
    use bridge_core::workflow_history::{
        DirectAttemptBarrier, DirectCleanupSettlement, LedgerError,
    };

    let _backend_liveness: fn(&dyn AgentBackend) -> AcpChildLiveness =
        |backend| backend.bridge_owned_acp_child_liveness();
    let _declare_capability: fn(&dyn TerminalEvidenceSink, EvidenceCapability) =
        |sink, capability| sink.declare_capability(capability);
    let _prepare_terminal_evidence: fn(
        &DirectAttemptBarrier,
        bridge_core::terminal_evidence::TurnEvidenceBinding,
    ) = DirectAttemptBarrier::prepare_terminal_evidence;
    let _record_activity: fn(
        &mut DirectAttemptBarrier,
        AttemptPhase,
        ActivityReason,
        u64,
    ) -> AttemptActivity = DirectAttemptBarrier::record_activity;
    let _seal_child_liveness: fn(&mut DirectAttemptBarrier, AcpChildLiveness) =
        DirectAttemptBarrier::seal_child_liveness;
    let _cleanup_settlement: fn(
        &DirectAttemptBarrier,
    ) -> Result<DirectCleanupSettlement, LedgerError> = DirectAttemptBarrier::cleanup_settlement;
}

#[test]
fn completed_without_a_final_is_protocol_incomplete() {
    let resolved = resolve_terminal(TerminalObservation {
        capability: EvidenceCapability::V1,
        completeness: EvidenceCompleteness::Complete,
        producer: ProducerTerminal::Completed,
        final_presence: FinalPresence::Absent,
        prompt_rpc: PromptRpcObservation::Resolved,
        ordered_notifications_drained: true,
        deliverable_final_present: false,
        child_liveness: AcpChildLiveness::Live,
    });

    assert_eq!(resolved.outcome.as_str(), "failed");
    assert_eq!(resolved.reason.as_str(), "protocol_incomplete_final");
    assert_eq!(resolved.producer, ProducerTerminal::Completed);
    assert_eq!(resolved.final_presence, FinalPresence::Absent);
    assert_eq!(resolved.child_liveness, AcpChildLiveness::Live);
}

#[test]
fn child_liveness_never_changes_producer_disposition() {
    for child_liveness in [
        AcpChildLiveness::Unknown,
        AcpChildLiveness::Live,
        AcpChildLiveness::Exited,
    ] {
        let resolved = resolve_terminal(TerminalObservation {
            capability: EvidenceCapability::Unsupported,
            completeness: EvidenceCompleteness::Unsupported,
            producer: ProducerTerminal::Unknown,
            final_presence: FinalPresence::Unknown,
            prompt_rpc: PromptRpcObservation::RejectedAcceptedOrUncertain,
            ordered_notifications_drained: false,
            deliverable_final_present: false,
            child_liveness,
        });
        assert_eq!(resolved.reason.as_str(), "protocol_terminal_unknown");
        assert_eq!(resolved.producer, ProducerTerminal::Unknown);
        assert_eq!(resolved.child_liveness, child_liveness);
    }
}

#[test]
fn producer_final_and_child_liveness_cross_product_keeps_independent_authorities() {
    for producer in [ProducerTerminal::Failed, ProducerTerminal::Interrupted] {
        for final_presence in [
            FinalPresence::Unknown,
            FinalPresence::Nonempty,
            FinalPresence::Absent,
        ] {
            for prompt_rpc in [
                PromptRpcObservation::Resolved,
                PromptRpcObservation::RejectedBeforeAcceptance,
                PromptRpcObservation::RejectedAcceptedOrUncertain,
            ] {
                for child_liveness in [
                    AcpChildLiveness::Unknown,
                    AcpChildLiveness::Live,
                    AcpChildLiveness::Exited,
                ] {
                    let resolved = resolve_terminal(TerminalObservation {
                        capability: EvidenceCapability::V1,
                        completeness: EvidenceCompleteness::Complete,
                        producer,
                        final_presence,
                        prompt_rpc,
                        ordered_notifications_drained: true,
                        deliverable_final_present: final_presence == FinalPresence::Nonempty,
                        child_liveness,
                    });
                    let expected = match producer {
                        ProducerTerminal::Failed => "producer_failed",
                        ProducerTerminal::Interrupted => "producer_interrupted",
                        _ => unreachable!(),
                    };
                    assert_eq!(resolved.reason.as_str(), expected);
                    assert_eq!(resolved.producer, producer);
                    assert_eq!(resolved.final_presence, final_presence);
                    assert_eq!(resolved.child_liveness, child_liveness);
                }
            }
        }
    }

    for child_liveness in [
        AcpChildLiveness::Unknown,
        AcpChildLiveness::Live,
        AcpChildLiveness::Exited,
    ] {
        let completed = resolve_terminal(TerminalObservation {
            capability: EvidenceCapability::V1,
            completeness: EvidenceCompleteness::Complete,
            producer: ProducerTerminal::Completed,
            final_presence: FinalPresence::Nonempty,
            prompt_rpc: PromptRpcObservation::Resolved,
            ordered_notifications_drained: true,
            deliverable_final_present: true,
            child_liveness,
        });
        assert_eq!(completed.outcome.as_str(), "completed");
        assert_eq!(completed.reason.as_str(), "completed_final");

        for prompt_rpc in [
            PromptRpcObservation::Resolved,
            PromptRpcObservation::RejectedBeforeAcceptance,
            PromptRpcObservation::RejectedAcceptedOrUncertain,
        ] {
            let absent = resolve_terminal(TerminalObservation {
                capability: EvidenceCapability::V1,
                completeness: EvidenceCompleteness::Complete,
                producer: ProducerTerminal::Completed,
                final_presence: FinalPresence::Absent,
                prompt_rpc,
                ordered_notifications_drained: true,
                deliverable_final_present: false,
                child_liveness,
            });
            assert_eq!(absent.outcome.as_str(), "failed");
            assert_eq!(absent.reason.as_str(), "protocol_incomplete_final");
        }
    }
}

#[tokio::test]
async fn direct_completed_evidence_without_model_text_is_delivery_conflict() {
    use std::sync::Arc;

    use bridge_core::ids::{AttemptIdentity, TaskId};
    use bridge_core::terminal_evidence::{
        EvidenceAcceptance, TurnEvidenceBinding, TurnEvidenceEnvelope, TURN_EVIDENCE_VERSION,
    };
    use bridge_core::workflow_history::{
        AttemptReservation, DirectAttemptBarrier, ExecutionSurface, MemoryWorkflowHistoryStore,
    };

    let identity = AttemptIdentity::initial().unwrap();
    let task_id = TaskId::parse(identity.execution_id.as_str().to_owned()).unwrap();
    let store = Arc::new(MemoryWorkflowHistoryStore::new());
    let mut barrier = DirectAttemptBarrier::admit(
        store,
        AttemptReservation {
            identity: identity.clone(),
            task_id: Some(task_id),
            workflow: "direct".into(),
            task_class: "direct".into(),
            surface: ExecutionSurface::DirectUnary,
            policy: "r2f0b".into(),
            workload_fingerprint: "delivery-conflict".into(),
            started_ms: 1,
            workload_fingerprint_complete: true,
            prompt_acceptance: "not_dispatched".into(),
            pinned: false,
        },
        "caller_aborted",
    )
    .await
    .unwrap();
    let binding = TurnEvidenceBinding {
        generation: 1,
        session_id: "bridge-session".into(),
        turn_id: "turn-delivery".into(),
        attempt_id: identity.attempt_id.as_str().into(),
        marker_nonce: "00112233445566778899aabbccddeeff".into(),
    };
    barrier.configure_terminal_evidence(binding.clone());
    barrier.mark_prompt_dispatch().await.unwrap();
    assert_eq!(
        barrier
            .terminal_evidence_sink()
            .accept(TurnEvidenceEnvelope {
                version: TURN_EVIDENCE_VERSION.into(),
                generation: binding.generation,
                session_id: binding.session_id,
                turn_id: binding.turn_id,
                attempt_id: binding.attempt_id,
                marker_nonce: binding.marker_nonce,
                native_turn_id: "native-turn".into(),
                sequence: 1,
                producer: ProducerTerminal::Completed,
                final_presence: FinalPresence::Nonempty,
                ordered_notifications_drained: true,
                complete: true,
            }),
        EvidenceAcceptance::Accepted,
    );

    let (_, terminal) = barrier
        .finish("completed", "completed", false, "complete", true)
        .await
        .unwrap();
    assert_eq!(terminal.outcome, "failed");
    assert_eq!(terminal.terminal_reason, "protocol_final_delivery_conflict");
}

#[tokio::test]
async fn missing_v1_evidence_and_pending_cleanup_are_truthful_and_settle_once() {
    use bridge_core::ids::{AttemptIdentity, TaskId};
    use bridge_core::workflow_history::{
        AttemptReservation, AttemptTerminal, ExecutionSurface, MemoryWorkflowHistoryStore,
        NodeCounts, TerminalWrite, WorkflowHistoryStore,
    };

    let identity = AttemptIdentity::initial().unwrap();
    let task_id = TaskId::parse(identity.execution_id.as_str().to_owned()).unwrap();
    let store = MemoryWorkflowHistoryStore::new();
    store
        .reserve(&AttemptReservation {
            identity: identity.clone(),
            task_id: Some(task_id),
            workflow: "direct".into(),
            task_class: "direct".into(),
            surface: ExecutionSurface::DirectUnary,
            policy: "r2f0b".into(),
            workload_fingerprint: "shape-test".into(),
            started_ms: 1,
            workload_fingerprint_complete: false,
            prompt_acceptance: "dispatch_uncertain".into(),
            pinned: false,
        })
        .await
        .unwrap();
    let terminal = AttemptTerminal {
        completed_ms: 2,
        work_ms: 0,
        end_to_end_ms: 1,
        queue_ms: 0,
        cancellation_ms: 0,
        cleanup_ms: 0,
        finalization_ms: 0,
        outcome: "failed".into(),
        terminal_reason: "protocol_terminal_evidence_missing".into(),
        producer_terminal: "unknown".into(),
        final_message: "unknown".into(),
        process_liveness: "live".into(),
        terminal_evidence_capability: "v1".into(),
        terminal_evidence_version: "v1".into(),
        terminal_evidence_source: "none".into(),
        terminal_evidence_complete: false,
        terminal_evidence_counts: Default::default(),
        degraded: true,
        prompt_acceptance: "dispatch_uncertain".into(),
        cleanup_disposition: "pending".into(),
        node_counts: NodeCounts::default(),
        phase_durations: Vec::new(),
        telemetry_complete: false,
        monotonic_clock: true,
    };
    assert_eq!(
        store
            .terminalize(&identity.attempt_id, &terminal)
            .await
            .unwrap(),
        TerminalWrite::Applied
    );
    assert_eq!(
        store
            .settle_cleanup(&identity.attempt_id, "complete")
            .await
            .unwrap(),
        TerminalWrite::Applied
    );
    assert_eq!(
        store
            .settle_cleanup(&identity.attempt_id, "complete")
            .await
            .unwrap(),
        TerminalWrite::Replayed
    );
    assert_eq!(
        store
            .settle_cleanup(&identity.attempt_id, "failed")
            .await
            .unwrap(),
        TerminalWrite::Conflict
    );
}

fn dormant_binding(label: &str) -> bridge_core::terminal_evidence::TurnEvidenceBinding {
    bridge_core::terminal_evidence::TurnEvidenceBinding {
        generation: 9,
        session_id: format!("session-{label}"),
        turn_id: format!("turn-{label}"),
        attempt_id: format!("attempt-{label}"),
        marker_nonce: "00112233445566778899aabbccddeeff".into(),
    }
}

fn exact_envelope(
    binding: bridge_core::terminal_evidence::TurnEvidenceBinding,
) -> bridge_core::terminal_evidence::TurnEvidenceEnvelope {
    bridge_core::terminal_evidence::TurnEvidenceEnvelope {
        version: bridge_core::terminal_evidence::TURN_EVIDENCE_VERSION.into(),
        generation: binding.generation,
        session_id: binding.session_id,
        turn_id: binding.turn_id,
        attempt_id: binding.attempt_id,
        marker_nonce: binding.marker_nonce,
        native_turn_id: "native-turn".into(),
        sequence: 1,
        producer: ProducerTerminal::Completed,
        final_presence: FinalPresence::Nonempty,
        ordered_notifications_drained: true,
        complete: true,
    }
}

#[test]
fn dormant_binding_promotes_on_exact_v1_declaration_and_accepts_envelope() {
    use bridge_core::terminal_evidence::{
        EvidenceAcceptance, SharedTurnEvidence, TerminalEvidenceSink,
    };

    let binding = dormant_binding("promote");
    let sink = SharedTurnEvidence::dormant(binding.clone());
    assert_eq!(sink.capability(), EvidenceCapability::Unsupported);
    assert_eq!(sink.binding(), Some(binding.clone()));

    sink.declare_capability(EvidenceCapability::V1);
    assert_eq!(sink.capability(), EvidenceCapability::V1);
    assert_eq!(
        sink.accept(exact_envelope(binding)),
        EvidenceAcceptance::Accepted
    );
    assert_eq!(sink.observation().0, EvidenceCompleteness::Complete);
}

#[test]
fn terminal_declaration_negative_edges_remain_conservative() {
    use bridge_core::terminal_evidence::{SharedTurnEvidence, TerminalEvidenceSink};

    let unsupported = SharedTurnEvidence::dormant(dormant_binding("unsupported"));
    unsupported.declare_capability(EvidenceCapability::Unsupported);
    unsupported.close();
    assert_eq!(unsupported.capability(), EvidenceCapability::Unsupported);
    assert_eq!(
        unsupported.observation().0,
        EvidenceCompleteness::Unsupported
    );

    let malformed = SharedTurnEvidence::dormant(dormant_binding("malformed"));
    malformed.declare_capability(EvidenceCapability::MalformedAdvertisement);
    assert_eq!(malformed.capability(), EvidenceCapability::V1);
    assert_eq!(malformed.observation().0, EvidenceCompleteness::Malformed);

    let missing = SharedTurnEvidence::unsupported();
    missing.declare_capability(EvidenceCapability::V1);
    assert_eq!(missing.capability(), EvidenceCapability::V1);
    assert_eq!(missing.observation().0, EvidenceCompleteness::Malformed);

    let mut invalid_binding = dormant_binding("invalid");
    invalid_binding.session_id.clear();
    let invalid = SharedTurnEvidence::dormant(invalid_binding);
    invalid.declare_capability(EvidenceCapability::V1);
    assert_eq!(invalid.observation().0, EvidenceCompleteness::Malformed);
}

#[test]
fn declarations_are_idempotent_and_cannot_reset_accepted_sticky_or_closed_state() {
    use bridge_core::terminal_evidence::{
        EvidenceAcceptance, SharedTurnEvidence, TerminalEvidenceSink,
    };

    let binding = dormant_binding("accepted");
    let accepted = SharedTurnEvidence::dormant(binding.clone());
    accepted.declare_capability(EvidenceCapability::V1);
    assert_eq!(
        accepted.accept(exact_envelope(binding.clone())),
        EvidenceAcceptance::Accepted
    );
    accepted.declare_capability(EvidenceCapability::V1);
    assert_eq!(accepted.observation().0, EvidenceCompleteness::Complete);
    accepted.declare_capability(EvidenceCapability::Unsupported);
    assert_eq!(accepted.observation().0, EvidenceCompleteness::Conflict);
    assert_eq!(
        accepted.accept(exact_envelope(binding)),
        EvidenceAcceptance::Rejected(EvidenceCompleteness::Conflict)
    );

    let sticky = SharedTurnEvidence::dormant(dormant_binding("sticky"));
    sticky.declare_capability(EvidenceCapability::MalformedAdvertisement);
    sticky.declare_capability(EvidenceCapability::V1);
    assert_eq!(sticky.observation().0, EvidenceCompleteness::Malformed);

    let closed = SharedTurnEvidence::dormant(dormant_binding("closed"));
    closed.declare_capability(EvidenceCapability::V1);
    closed.close();
    assert_eq!(closed.observation().0, EvidenceCompleteness::Missing);
    closed.declare_capability(EvidenceCapability::V1);
    closed.declare_capability(EvidenceCapability::Unsupported);
    assert_eq!(closed.observation().0, EvidenceCompleteness::Missing);
}

#[test]
fn exact_child_liveness_dominates_unknown_and_accepts_later_exact_update() {
    use bridge_core::terminal_evidence::{SharedTurnEvidence, TerminalEvidenceSink};

    let sink = SharedTurnEvidence::unsupported();
    sink.record_child_liveness(AcpChildLiveness::Live);
    sink.record_child_liveness(AcpChildLiveness::Unknown);
    assert_eq!(sink.child_liveness(), AcpChildLiveness::Live);
    sink.record_child_liveness(AcpChildLiveness::Exited);
    sink.record_child_liveness(AcpChildLiveness::Unknown);
    assert_eq!(sink.child_liveness(), AcpChildLiveness::Exited);
}

#[tokio::test]
async fn direct_barrier_projects_retained_sink_liveness_after_decorator_unknown() {
    use std::sync::Arc;

    use bridge_core::ids::{AttemptIdentity, TaskId};
    use bridge_core::workflow_history::{
        AttemptReservation, DirectAttemptBarrier, ExecutionSurface, MemoryWorkflowHistoryStore,
    };

    let identity = AttemptIdentity::initial().unwrap();
    let task_id = TaskId::parse(identity.execution_id.as_str().to_owned()).unwrap();
    let mut barrier = DirectAttemptBarrier::admit(
        Arc::new(MemoryWorkflowHistoryStore::new()),
        AttemptReservation {
            identity,
            task_id: Some(task_id),
            workflow: "direct".into(),
            task_class: "direct".into(),
            surface: ExecutionSurface::DirectUnary,
            policy: "r2f0b-repair".into(),
            workload_fingerprint: "retained-child-liveness".into(),
            started_ms: 1,
            workload_fingerprint_complete: true,
            prompt_acceptance: "not_dispatched".into(),
            pinned: false,
        },
        "caller_aborted",
    )
    .await
    .unwrap();
    barrier
        .terminal_evidence_sink()
        .record_child_liveness(AcpChildLiveness::Live);
    barrier.seal_child_liveness(AcpChildLiveness::Unknown);

    let (_, terminal) = barrier
        .finish("completed", "completed", false, "complete", true)
        .await
        .unwrap();
    assert_eq!(
        terminal.process_liveness, "live",
        "the decorator's ambiguous sample must not erase exact sink liveness"
    );
}
