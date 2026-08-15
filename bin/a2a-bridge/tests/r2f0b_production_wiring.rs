use bridge_api::{ApiBackend, ApiConfig};
use bridge_core::attempt_activity::{
    ActivityKind, ActivityReason, AttemptPhase, AttemptTelemetrySinkFactory,
};
use bridge_core::diagnostics::NoopDiagnosticObserver;
use bridge_core::domain::Part;
use bridge_core::ids::{AttemptIdentity, NodeId, SessionId, TaskId};
use bridge_core::ports::{AgentBackend, BackendObservers, RichEventSinkFactory};
use bridge_core::terminal_evidence::{
    AcpChildLiveness, EvidenceAcceptance, EvidenceCapability, EvidenceCompleteness, FinalPresence,
    ProducerTerminal, TurnEvidenceEnvelope, TURN_EVIDENCE_VERSION,
};
use bridge_core::workflow_history::{
    AttemptReservation, DirectAttemptBarrier, ExecutionSurface, MemoryWorkflowHistoryStore,
};
use futures::StreamExt;
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn task_g_production_api_construction_explicitly_leaves_v3_unarmed() {
    let source = include_str!("../src/main.rs");
    let api_start = source
        .find("AgentKind::Api => {")
        .expect("production API construction branch");
    let container_start = source[api_start..]
        .find("AgentKind::ContainerRw => {")
        .map(|offset| api_start + offset)
        .expect("branch following production API construction");
    let api_branch = &source[api_start..container_start];

    assert!(
        api_branch.contains("api_cfg.resource_flight_route_v3 = None;"),
        "production API construction must explicitly assign the V3 route to None"
    );
    assert!(
        !api_branch.contains("api_cfg.resource_flight_route_v3 = Some"),
        "Task G must not arm the production V3 route"
    );
}

#[test]
fn production_attempt_factory_carries_activity_and_terminal_evidence_to_one_owner() {
    let factory = AttemptTelemetrySinkFactory::new("attempt-production");
    let rich = factory.make(&NodeId::parse("only").unwrap());
    let recorder = rich.attempt_recorder().expect("production recorder");
    let activity = recorder
        .record(AttemptPhase::Provider, ActivityReason::MessageDelta, 4)
        .expect("production recorder returns its observation");
    assert_eq!(activity.kind, ActivityKind::MeaningfulProgress);

    let sink = rich
        .terminal_evidence_for_turn(EvidenceCapability::V1, 7, "bridge-session", "bridge-turn")
        .unwrap()
        .expect("production evidence sink");
    let binding = sink.binding().expect("negotiated turn binding");
    assert_eq!(
        sink.accept(TurnEvidenceEnvelope {
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
    sink.record_child_liveness(AcpChildLiveness::Live);
    sink.record_deliverable_final();
    sink.close();

    let (
        capability,
        completeness,
        producer,
        final_presence,
        drained,
        child_liveness,
        deliverable_final_present,
    ) = factory
        .evidence()
        .single_turn()
        .expect("one terminal owner");
    assert_eq!(capability, EvidenceCapability::V1);
    assert_eq!(completeness, EvidenceCompleteness::Complete);
    assert_eq!(producer, ProducerTerminal::Completed);
    assert_eq!(final_presence, FinalPresence::Nonempty);
    assert!(drained);
    assert_eq!(child_liveness, AcpChildLiveness::Live);
    assert!(deliverable_final_present);
}

#[test]
fn independent_turn_scopes_advance_resetting_progress_without_accepting_replays() {
    let factory = AttemptTelemetrySinkFactory::new("attempt-independent-turns");
    let first = factory.make(&NodeId::parse("first").unwrap());
    let second = factory.make(&NodeId::parse("second").unwrap());
    let first = first.attempt_recorder().expect("first turn recorder");
    let second = second.attempt_recorder().expect("second turn recorder");

    for (phase, reason) in [
        (AttemptPhase::Provider, ActivityReason::MessageDelta),
        (AttemptPhase::Provider, ActivityReason::ThoughtDelta),
        (AttemptPhase::Provider, ActivityReason::UsageHighWater),
        (AttemptPhase::Tool, ActivityReason::ToolTransition),
    ] {
        assert_eq!(
            first.record(phase, reason, 100).unwrap().kind,
            ActivityKind::MeaningfulProgress
        );
        assert_eq!(
            second.record(phase, reason, 1).unwrap().kind,
            ActivityKind::MeaningfulProgress,
            "a genuine nonempty later-turn advance must not collide with the first turn"
        );
        assert_eq!(
            second.record(phase, reason, 1).unwrap().kind,
            ActivityKind::Activity,
            "an exact replay within one turn remains activity only"
        );
        assert_eq!(
            second.record(phase, reason, 0).unwrap().kind,
            ActivityKind::Activity,
            "a decreasing or empty observation within one turn remains activity only"
        );
    }

    let tally = factory.recorder().tally().unwrap();
    assert_eq!(tally.meaningful_progress, 8);
    assert_eq!(tally.activity, 8);
    assert!(
        tally.encoded_len() <= bridge_core::attempt_activity::MAX_ATTACHMENT_ENCODING_BYTES,
        "the persisted attempt tally remains bounded and low-cardinality"
    );
}

async fn local_streaming_api(text: &str) -> (MockServer, ApiBackend) {
    let server = MockServer::start().await;
    let body = format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":{text:?}}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let backend = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri())));
    (server, backend)
}

async fn drain_api(backend: &ApiBackend, session: &str, observers: BackendObservers) {
    let mut stream = backend
        .prompt_with_observers(
            &SessionId::parse(session).unwrap(),
            vec![Part { text: "hi".into() }],
            observers,
        )
        .await
        .unwrap();
    while let Some(update) = stream.next().await {
        update.unwrap();
    }
}

#[tokio::test]
async fn real_api_message_delta_reaches_production_direct_attempt_owner() {
    let (_server, backend) = local_streaming_api("direct").await;
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
            workload_fingerprint: "real-api-direct".into(),
            started_ms: 1,
            workload_fingerprint_complete: true,
            prompt_acceptance: "not_dispatched".into(),
            pinned: false,
        },
        "caller_aborted",
    )
    .await
    .unwrap();
    barrier.mark_prompt_dispatch().await.unwrap();
    let recorder = barrier.activity_recorder();
    let before = recorder.tally().unwrap();
    drain_api(
        &backend,
        "direct-api-owner",
        BackendObservers::diagnostic_only(Arc::new(NoopDiagnosticObserver::default()))
            .with_attempt_telemetry(recorder.clone(), barrier.terminal_evidence_sink()),
    )
    .await;
    let after = recorder.tally().unwrap();
    assert_eq!(after.meaningful_progress, before.meaningful_progress + 1);
    assert_eq!(after.max_advance, 6);
    barrier
        .finish("completed", "completed", false, "complete", true)
        .await
        .unwrap();
}

#[tokio::test]
async fn real_api_message_delta_reaches_production_workflow_attempt_owner() {
    let (_server, backend) = local_streaming_api("workflow").await;
    let factory = AttemptTelemetrySinkFactory::new("real-api-workflow");
    let rich = factory.make(&NodeId::parse("api-node").unwrap());
    let recorder = rich.attempt_recorder().expect("production recorder");
    let evidence = rich
        .terminal_evidence_for_turn(
            backend.terminal_evidence_capability(),
            0,
            "workflow-api-owner",
            "turn-api-owner",
        )
        .unwrap()
        .expect("production terminal owner");
    drain_api(
        &backend,
        "workflow-api-owner",
        BackendObservers::new(Arc::new(NoopDiagnosticObserver::default()), Some(rich))
            .with_attempt_telemetry(recorder.clone(), evidence),
    )
    .await;
    let tally = recorder.tally().unwrap();
    assert_eq!(tally.meaningful_progress, 1);
    assert_eq!(tally.max_advance, 8);
}
