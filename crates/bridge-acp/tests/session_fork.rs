// session_fork.rs — ACP `session/fork` through the real `AcpBackend`.
//
// The fake adapter below speaks the real JSON-RPC shape: its `session/fork` handler is the SDK's typed
// `ForkSessionRequest`, so a request the backend got wrong on the wire would not reach it.

use agent_client_protocol::{
    schema::{v1::*, ProtocolVersion},
    Agent, Channel, Client, ConnectionTo, Responder,
};
use bridge_acp::acp_backend::{AcpBackend, AcpConfig, ContainerReap};
use bridge_core::{
    domain::{EffectiveConfig, Part, SessionSpec},
    error::{BridgeError, ForkRefusal},
    ids::SessionId,
    ports::AgentBackend,
    session_cwd::SessionCwd,
};
use futures::StreamExt;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;

/// What the fake adapter saw, in wire order.
struct Fake {
    advertise_fork: bool,
    log: Mutex<Vec<String>>,
    fork_params: Mutex<Vec<serde_json::Value>>,
    prompt_sessions: Mutex<Vec<String>>,
    /// Session id -> (model, mode) the adapter currently holds for it.
    state: Mutex<HashMap<String, (String, String)>>,
    gated: std::sync::atomic::AtomicBool,
    gate: Semaphore,
}

impl Fake {
    fn new(advertise_fork: bool) -> Self {
        Self {
            advertise_fork,
            log: Mutex::default(),
            fork_params: Mutex::default(),
            prompt_sessions: Mutex::default(),
            state: Mutex::default(),
            gated: std::sync::atomic::AtomicBool::new(false),
            gate: Semaphore::new(0),
        }
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn entries(&self, prefix: &str) -> Vec<String> {
        self.log()
            .into_iter()
            .filter(|entry| entry.starts_with(prefix))
            .collect()
    }
    fn gate_prompts(&self) {
        self.gated.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    fn release_prompts(&self, n: usize) {
        self.gate.add_permits(n);
    }
}

fn options(model: &str) -> serde_json::Value {
    json!([{
        "id": "model", "name": "model", "category": "model", "type": "select",
        "currentValue": model,
        "options": [{"value": "model", "name": "model"}, {"value": "good", "name": "good"}]
    }])
}

fn modes(mode: &str) -> serde_json::Value {
    json!({"currentModeId": mode,
           "availableModes": [{"id": "code", "name": "Code"}, {"id": "ask", "name": "Ask"}]})
}

async fn connect(fake: Fake) -> (Arc<Fake>, Arc<AcpBackend>) {
    connect_with(fake, AcpConfig::default()).await
}

async fn connect_with(fake: Fake, config: AcpConfig) -> (Arc<Fake>, Arc<AcpBackend>) {
    let rec = Arc::new(fake);
    let (client, agent) = Channel::duplex();
    let (init, new, fork, mode, config_option, prompt, cancel, close) = (
        rec.clone(),
        rec.clone(),
        rec.clone(),
        rec.clone(),
        rec.clone(),
        rec.clone(),
        rec.clone(),
        rec.clone(),
    );
    tokio::spawn(async move {
        Agent
            .builder()
            .name("fork-agent")
            .on_receive_request(
                move |_: InitializeRequest, r: Responder<InitializeResponse>, _| {
                    let rec = init.clone();
                    async move {
                        let mut caps = AgentCapabilities::new();
                        if rec.advertise_fork {
                            caps = caps.session_capabilities(
                                SessionCapabilities::new().fork(SessionForkCapabilities::new()),
                            );
                        }
                        r.respond(InitializeResponse::new(ProtocolVersion::V1).agent_capabilities(caps))?;
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                move |_: NewSessionRequest, r: Responder<NewSessionResponse>, _| {
                    let rec = new.clone();
                    async move {
                        let mut state = rec.state.lock().unwrap();
                        let id = format!("mint-{}", rec.entries("new").len());
                        rec.log.lock().unwrap().push(format!("new:{id}"));
                        state.insert(id.clone(), ("model".into(), "code".into()));
                        let response: NewSessionResponse = serde_json::from_value(json!({
                            "sessionId": id, "configOptions": options("model"), "modes": modes("code")
                        }))
                        .unwrap();
                        r.respond(response)?;
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                move |req: ForkSessionRequest, r: Responder<ForkSessionResponse>, _| {
                    let rec = fork.clone();
                    async move {
                        rec.fork_params
                            .lock()
                            .unwrap()
                            .push(serde_json::to_value(&req).unwrap());
                        let mut state = rec.state.lock().unwrap();
                        let source = req.session_id.0.to_string();
                        let Some(held) = state.get(&source).cloned() else {
                            r.respond_with_internal_error("unknown source session")?;
                            return Ok(());
                        };
                        let id = format!("fork-{}", rec.entries("fork").len());
                        rec.log.lock().unwrap().push(format!("fork:{source}->{id}"));
                        // A fork starts like a resumed session: with the model and mode the source holds.
                        let response: ForkSessionResponse = serde_json::from_value(json!({
                            "sessionId": id, "configOptions": options(&held.0), "modes": modes(&held.1)
                        }))
                        .unwrap();
                        state.insert(id, held);
                        r.respond(response)?;
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                move |req: SetSessionModeRequest, r: Responder<SetSessionModeResponse>, _| {
                    let rec = mode.clone();
                    async move {
                        let id = req.session_id.0.to_string();
                        rec.log
                            .lock()
                            .unwrap()
                            .push(format!("set_mode:{id}:{}", req.mode_id.0));
                        rec.state.lock().unwrap().get_mut(&id).unwrap().1 = req.mode_id.0.to_string();
                        r.respond(SetSessionModeResponse::new())?;
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                move |req: SetSessionConfigOptionRequest,
                      r: Responder<SetSessionConfigOptionResponse>,
                      _| {
                    let rec = config_option.clone();
                    async move {
                        let id = req.session_id.0.to_string();
                        rec.log.lock().unwrap().push(format!(
                            "set_config:{id}:{}:{}",
                            req.config_id.0, req.value.0
                        ));
                        let mut state = rec.state.lock().unwrap();
                        state.get_mut(&id).unwrap().0 = req.value.0.to_string();
                        let options: Vec<SessionConfigOption> =
                            serde_json::from_value(options(&req.value.0)).unwrap();
                        r.respond(SetSessionConfigOptionResponse::new(options))?;
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                move |req: PromptRequest, r: Responder<PromptResponse>, cx: ConnectionTo<Client>| {
                    let rec = prompt.clone();
                    async move {
                        let id = req.session_id.0.to_string();
                        rec.log.lock().unwrap().push(format!("prompt:{id}"));
                        rec.prompt_sessions.lock().unwrap().push(id.clone());
                        let gated = rec.gated.load(std::sync::atomic::Ordering::SeqCst);
                        let connection = cx.clone();
                        cx.spawn(async move {
                            if gated {
                                rec.gate.acquire().await.unwrap().forget();
                            }
                            connection.send_notification(SessionNotification::new(
                                req.session_id,
                                SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                    ContentBlock::Text(TextContent::new(format!("answer from {id}"))),
                                )),
                            ))?;
                            r.respond(PromptResponse::new(StopReason::EndTurn))?;
                            Ok(())
                        })?;
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                move |req: CancelNotification, _| {
                    let rec = cancel.clone();
                    async move {
                        rec.log.lock().unwrap().push(format!("cancel:{}", req.session_id.0));
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                move |req: CloseSessionRequest, r: Responder<CloseSessionResponse>, _| {
                    let rec = close.clone();
                    async move {
                        rec.log.lock().unwrap().push(format!("close:{}", req.session_id.0));
                        r.respond(CloseSessionResponse::new())?;
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_to(agent)
            .await
            .unwrap();
    });
    let backend = AcpBackend::connect(
        client,
        AcpConfig {
            cwd: std::env::temp_dir(),
            ..config
        },
    )
    .await
    .unwrap();
    (rec, Arc::new(backend))
}

fn fork_fake() -> Fake {
    Fake::new(true)
}

fn key(name: &str) -> SessionId {
    SessionId::parse(name).unwrap()
}

async fn configure(backend: &AcpBackend, session: &SessionId, config: EffectiveConfig) {
    backend
        .configure_session(session, &SessionSpec { config, cwd: None })
        .await
        .unwrap();
}

/// Run one turn to its terminal event and return the text the adapter streamed.
async fn turn(backend: &AcpBackend, session: &SessionId) -> String {
    let mut stream = backend
        .prompt(session, vec![Part { text: "go".into() }])
        .await
        .unwrap();
    let mut text = String::new();
    while let Some(update) = stream.next().await {
        if let bridge_core::ports::Update::Text(chunk) = update.unwrap() {
            text.push_str(&chunk);
        }
    }
    text
}

/// A started source session: configured with the default config and one settled turn.
async fn started_source(backend: &AcpBackend, name: &str) -> SessionId {
    let source = key(name);
    configure(backend, &source, EffectiveConfig::default()).await;
    assert_eq!(turn(backend, &source).await, "answer from mint-0");
    source
}

fn refusal(error: &BridgeError) -> ForkRefusal {
    match error {
        BridgeError::ForkRefused { reason } => *reason,
        other => panic!("expected a fork refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn fork_sends_one_session_fork_with_the_source_session_id_and_prompts_the_returned_session() {
    let (fake, backend) = connect(fork_fake()).await;
    let source = started_source(&backend, "src").await;
    let target = key("tgt");
    configure(&backend, &target, EffectiveConfig::default()).await;

    backend.fork_support().expect("adapter advertises fork");
    backend.fork_session(&source, &target).await.unwrap();

    assert_eq!(fake.entries("fork"), ["fork:mint-0->fork-0"]);
    let params = fake.fork_params.lock().unwrap().clone();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0]["sessionId"], "mint-0");
    assert!(params[0]["cwd"]
        .as_str()
        .is_some_and(|cwd| cwd.starts_with('/')));
    assert!(
        fake.entries("new").len() == 1,
        "the target is minted by the fork, not by session/new: {:?}",
        fake.log()
    );

    assert_eq!(turn(&backend, &target).await, "answer from fork-0");
    assert_eq!(
        fake.prompt_sessions.lock().unwrap().clone(),
        ["mint-0", "fork-0"]
    );
}

#[tokio::test]
async fn forks_and_their_source_each_prompt_only_their_own_session() {
    let (fake, backend) = connect(fork_fake()).await;
    let source = started_source(&backend, "src").await;
    let (a, b) = (key("fork-a"), key("fork-b"));
    for target in [&a, &b] {
        configure(&backend, target, EffectiveConfig::default()).await;
        backend.fork_session(&source, target).await.unwrap();
    }
    assert_eq!(
        fake.entries("fork"),
        ["fork:mint-0->fork-0", "fork:mint-0->fork-1"]
    );

    assert_eq!(turn(&backend, &a).await, "answer from fork-0");
    assert_eq!(turn(&backend, &b).await, "answer from fork-1");
    assert_eq!(turn(&backend, &source).await, "answer from mint-0");
    assert_eq!(turn(&backend, &a).await, "answer from fork-0");
    assert_eq!(
        fake.prompt_sessions.lock().unwrap().clone(),
        ["mint-0", "fork-0", "fork-1", "mint-0", "fork-0"]
    );
}

#[tokio::test]
async fn two_forks_of_one_source_prompt_concurrently() {
    let (fake, backend) = connect(fork_fake()).await;
    let source = started_source(&backend, "src").await;
    let (a, b) = (key("fork-a"), key("fork-b"));
    for target in [&a, &b] {
        configure(&backend, target, EffectiveConfig::default()).await;
        backend.fork_session(&source, target).await.unwrap();
    }
    // Hold every reply until both prompts are on the wire: neither fork's turn may wait for the other.
    fake.gate_prompts();
    let (first, second) = (backend.clone(), backend.clone());
    let (a2, b2) = (a.clone(), b.clone());
    let run_a = tokio::spawn(async move { turn(&first, &a2).await });
    let run_b = tokio::spawn(async move { turn(&second, &b2).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while fake.prompt_sessions.lock().unwrap().len() < 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("both forks' prompts reach the adapter before either reply");
    fake.release_prompts(2);
    let mut answers = [run_a.await.unwrap(), run_b.await.unwrap()];
    answers.sort();
    assert_eq!(answers, ["answer from fork-0", "answer from fork-1"]);
}

#[tokio::test]
async fn forks_created_at_the_same_moment_are_all_created() {
    let (fake, backend) = connect(fork_fake()).await;
    let source = started_source(&backend, "src").await;
    let targets: Vec<_> = (0..4).map(|n| key(&format!("fork-{n}"))).collect();
    for target in &targets {
        configure(&backend, target, EffectiveConfig::default()).await;
    }
    // Sibling forks queue behind each other's `session/fork`; none mistakes another for a busy source.
    let results = futures::future::join_all(
        targets
            .iter()
            .map(|target| backend.fork_session(&source, target)),
    )
    .await;
    assert!(results.iter().all(Result::is_ok), "{results:?}");
    assert_eq!(fake.entries("fork").len(), 4);
    let mut forked: Vec<_> = fake.state.lock().unwrap().keys().cloned().collect();
    forked.sort();
    assert_eq!(forked, ["fork-0", "fork-1", "fork-2", "fork-3", "mint-0"]);
}

#[tokio::test]
async fn refusals_send_no_fork_and_no_prompt() {
    // Adapter without the capability.
    let (fake, backend) = connect(Fake::new(false)).await;
    let source = started_source(&backend, "src").await;
    let target = key("tgt");
    configure(&backend, &target, EffectiveConfig::default()).await;
    assert_eq!(backend.fork_support(), Err(ForkRefusal::AdapterUnsupported));
    let error = backend.fork_session(&source, &target).await.unwrap_err();
    assert_eq!(refusal(&error), ForkRefusal::AdapterUnsupported);
    assert!(fake.entries("fork").is_empty());
    assert_eq!(
        fake.prompt_sessions.lock().unwrap().len(),
        1,
        "only the source's own turn"
    );

    // Unknown, never-started, target-in-use and busy sources, on a capable adapter.
    let (fake, backend) = connect(fork_fake()).await;
    let target = key("tgt");
    configure(&backend, &target, EffectiveConfig::default()).await;
    let error = backend
        .fork_session(&key("nope"), &target)
        .await
        .unwrap_err();
    assert_eq!(refusal(&error), ForkRefusal::SourceNotFound);

    // A configured session that never took a turn has no agent session to fork.
    let unstarted = key("unstarted");
    configure(&backend, &unstarted, EffectiveConfig::default()).await;
    let error = backend.fork_session(&unstarted, &target).await.unwrap_err();
    assert_eq!(refusal(&error), ForkRefusal::SourceNotFound);

    let source = started_source(&backend, "src").await;
    let in_use = key("in-use");
    configure(&backend, &in_use, EffectiveConfig::default()).await;
    assert_eq!(turn(&backend, &in_use).await, "answer from mint-1");
    let error = backend.fork_session(&source, &in_use).await.unwrap_err();
    assert_eq!(refusal(&error), ForkRefusal::TargetInUse);

    fake.gate_prompts();
    let busy = backend.clone();
    let busy_source = source.clone();
    let running = tokio::spawn(async move { turn(&busy, &busy_source).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while fake.prompt_sessions.lock().unwrap().len() < 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the source's turn is in flight");
    let error = backend.fork_session(&source, &target).await.unwrap_err();
    assert_eq!(refusal(&error), ForkRefusal::SourceBusy);
    assert!(
        fake.entries("fork").is_empty(),
        "no refusal reached the adapter"
    );
    fake.release_prompts(1);
    running.await.unwrap();
    // The refused source is untouched: it forks once it is idle.
    backend.fork_session(&source, &target).await.unwrap();
    assert_eq!(fake.entries("fork").len(), 1);
}

#[tokio::test]
async fn a_containerized_reader_refuses_to_fork() {
    let reap = ContainerReap::from_legacy("docker", "reader", Arc::new(|_, _| {}));
    let (fake, backend) = connect_with(
        fork_fake(),
        AcpConfig {
            container: Some(reap),
            ..AcpConfig::default()
        },
    )
    .await;
    let source = started_source(&backend, "src").await;
    let target = key("tgt");
    configure(&backend, &target, EffectiveConfig::default()).await;
    let error = backend.fork_session(&source, &target).await.unwrap_err();
    assert_eq!(refusal(&error), ForkRefusal::BackendUnsupported);
    assert!(fake.entries("fork").is_empty());
}

#[tokio::test]
async fn per_message_model_and_mode_overrides_apply_to_the_forked_session() {
    let (fake, backend) = connect(fork_fake()).await;
    let source = started_source(&backend, "src").await;
    let target = key("tgt");
    configure(
        &backend,
        &target,
        EffectiveConfig {
            model: Some("good".into()),
            mode: Some("ask".into()),
            ..EffectiveConfig::default()
        },
    )
    .await;
    backend.fork_session(&source, &target).await.unwrap();

    let log = fake.log();
    let fork_at = log.iter().position(|e| e == "fork:mint-0->fork-0").unwrap();
    assert!(
        log[fork_at + 1..].contains(&"set_mode:fork-0:ask".to_owned())
            && log[fork_at + 1..].contains(&"set_config:fork-0:model:good".to_owned()),
        "the overrides follow the fork, on the forked session: {log:?}"
    );
    assert!(
        !log.iter()
            .any(|e| e.contains("mint-0:ask") || e.contains("mint-0:model:good")),
        "the source session keeps its own config: {log:?}"
    );
    assert_eq!(
        fake.state.lock().unwrap()["mint-0"],
        ("model".into(), "code".into())
    );
    assert_eq!(
        fake.state.lock().unwrap()["fork-0"],
        ("good".into(), "ask".into())
    );
}

#[tokio::test]
async fn the_forked_session_is_offered_the_target_cwd() {
    let (fake, backend) = connect(fork_fake()).await;
    let source = started_source(&backend, "src").await;
    let target = key("tgt");
    backend
        .configure_session(
            &target,
            &SessionSpec {
                config: EffectiveConfig::default(),
                cwd: Some(SessionCwd::parse("/work/other").unwrap()),
            },
        )
        .await
        .unwrap();
    backend.fork_session(&source, &target).await.unwrap();
    assert_eq!(fake.fork_params.lock().unwrap()[0]["cwd"], "/work/other");
}

#[tokio::test]
async fn cancelling_and_releasing_one_side_leaves_the_other_usable() {
    let (fake, backend) = connect(fork_fake()).await;
    let source = started_source(&backend, "src").await;
    let target = key("tgt");
    configure(&backend, &target, EffectiveConfig::default()).await;
    backend.fork_session(&source, &target).await.unwrap();

    // Cancelling the fork's turn cancels only the fork's agent session.
    fake.gate_prompts();
    let cancelled = backend.clone();
    let cancelled_target = target.clone();
    let running = tokio::spawn(async move {
        let mut stream = cancelled
            .prompt(&cancelled_target, vec![Part { text: "go".into() }])
            .await
            .unwrap();
        while stream.next().await.is_some() {}
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while fake.prompt_sessions.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    backend.cancel(&target).await.unwrap();
    fake.release_prompts(1);
    let _ = running.await;
    assert_eq!(fake.entries("cancel"), ["cancel:fork-0"]);

    // Releasing the source leaves the fork usable ...
    fake.gated.store(false, std::sync::atomic::Ordering::SeqCst);
    backend.release_session(&source).await;
    assert_eq!(turn(&backend, &target).await, "answer from fork-0");

    // ... and the source's release cancelled only the source's own agent session.
    assert_eq!(fake.entries("cancel"), ["cancel:fork-0", "cancel:mint-0"]);
    backend.release_session(&target).await;
}

#[tokio::test]
async fn releasing_the_fork_leaves_the_source_usable() {
    let (fake, backend) = connect(fork_fake()).await;
    let source = started_source(&backend, "src").await;
    let target = key("tgt");
    configure(&backend, &target, EffectiveConfig::default()).await;
    backend.fork_session(&source, &target).await.unwrap();
    assert_eq!(turn(&backend, &target).await, "answer from fork-0");
    backend.release_session(&target).await;
    assert!(
        !fake.entries("close").contains(&"close:mint-0".to_owned()),
        "releasing the fork never closes the source: {:?}",
        fake.log()
    );
    assert_eq!(turn(&backend, &source).await, "answer from mint-0");
}

#[tokio::test]
async fn describe_options_reports_fork_only_for_an_adapter_that_advertises_it() {
    for (fake, expected) in [(fork_fake(), true), (Fake::new(false), false)] {
        let (_fake, backend) = connect(fake).await;
        let caps = backend
            .describe_options(std::path::Path::new("/tmp"))
            .await
            .unwrap();
        assert_eq!(caps.fork, expected);
        let rendered = bridge_core::catalog::caps_to_json(&caps);
        assert_eq!(rendered.get("fork").is_some(), expected);
    }
}
