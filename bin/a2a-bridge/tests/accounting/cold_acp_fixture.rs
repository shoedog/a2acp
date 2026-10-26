//! Offline ACP peer: unlike Recording, selection belongs to minted agent sessions.
use agent_client_protocol::{
    schema::{v1::*, ProtocolVersion},
    Agent, Channel, Client, ConnectionTo, Responder,
};
use bridge_acp::acp_backend::{AcpBackend, AcpConfig};
use bridge_core::{domain::EffectiveConfig, ports::AgentBackend};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub struct ColdAcp {
    pub sessions: Mutex<HashMap<String, EffectiveConfig>>,
    pub prompts: Mutex<Vec<(String, EffectiveConfig)>>,
    pub teardown: Mutex<Vec<String>>,
    pub parts: Mutex<Vec<serde_json::Value>>,
    pub gates: Mutex<Vec<Option<tokio::sync::oneshot::Receiver<()>>>>,
}
fn options(config: &EffectiveConfig) -> Vec<SessionConfigOption> {
    [("model", "model", config.model.as_deref().unwrap_or("model"), vec!["model", "good"]),
     ("effort", "thought_level", if config.effort == Some(bridge_core::domain::Effort::Low) { "low" } else { "high" }, vec!["high", "low"])]
        .into_iter().map(|(id, category, current, values)| {
            serde_json::from_value(json!({"id":id,"name":id,"category":category,
                "type":"select","currentValue":current,
                "options":values.into_iter().map(|v| json!({"value":v,"name":v})).collect::<Vec<_>>()})).unwrap()
        }).collect()
}
impl ColdAcp {
    pub async fn connect() -> (Arc<Self>, Arc<AcpBackend>) {
        let rec = Arc::new(Self::default());
        let (client, agent) = Channel::duplex();
        let (new, config, mode, prompt, cancel, close) = (
            rec.clone(),
            rec.clone(),
            rec.clone(),
            rec.clone(),
            rec.clone(),
            rec.clone(),
        );
        tokio::spawn(async move {
            Agent.builder().name("cold-acp")
                .on_receive_request(|_: InitializeRequest, r: Responder<InitializeResponse>, _| async move {
                    r.respond(InitializeResponse::new(ProtocolVersion::V1))?; Ok(())
                }, agent_client_protocol::on_receive_request!())
                .on_receive_request(move |_: NewSessionRequest, r: Responder<NewSessionResponse>, _| {
                    let rec = new.clone(); async move {
                        let mut sessions = rec.sessions.lock().unwrap();
                        let id = format!("mint-{}", sessions.len());
                        let cfg = EffectiveConfig { model: Some("model".into()), ..Default::default() };
                        let response: NewSessionResponse = serde_json::from_value(json!({"sessionId":id,
                            "configOptions":options(&cfg),"modes":{"currentModeId":"code","availableModes":[{"id":"code","name":"Code"},{"id":"ask","name":"Ask"}]}})).unwrap();
                        sessions.insert(id, cfg);
                        r.respond(response)?; Ok(())
                    }
                }, agent_client_protocol::on_receive_request!())
                .on_receive_request(move |req: SetSessionConfigOptionRequest, r: Responder<SetSessionConfigOptionResponse>, _| {
                    let rec = config.clone(); async move {
                        let mut sessions = rec.sessions.lock().unwrap();
                        let cfg = sessions.get_mut(req.session_id.0.as_ref()).unwrap();
                        match req.config_id.0.as_ref() {
                            "model" => cfg.model = Some(req.value.0.to_string()),
                            "effort" => cfg.effort = Some(if req.value.0.as_ref() == "low" { bridge_core::domain::Effort::Low } else { bridge_core::domain::Effort::High }),
                            _ => panic!("unexpected config"),
                        }
                        r.respond(SetSessionConfigOptionResponse::new(options(cfg)))?; Ok(())
                    }
                }, agent_client_protocol::on_receive_request!())
                .on_receive_request(move |req: SetSessionModeRequest, r: Responder<SetSessionModeResponse>, _| {
                    let rec = mode.clone(); async move {
                        rec.sessions.lock().unwrap().get_mut(req.session_id.0.as_ref()).unwrap().mode = Some(req.mode_id.0.to_string());
                        r.respond(SetSessionModeResponse::new())?; Ok(())
                    }
                }, agent_client_protocol::on_receive_request!())
                .on_receive_request(move |req: PromptRequest, r: Responder<PromptResponse>, cx: ConnectionTo<Client>| {
                    let rec = prompt.clone(); async move {
                        rec.parts.lock().unwrap().push(serde_json::to_value(&req.prompt).unwrap());
                        let cfg = rec.sessions.lock().unwrap()[req.session_id.0.as_ref()].clone();
                        let index = { let mut prompts = rec.prompts.lock().unwrap(); prompts.push((req.session_id.0.to_string(), cfg)); prompts.len() - 1 };
                        let gate = rec.gates.lock().unwrap().get_mut(index).and_then(Option::take);
                        let connection = cx.clone();
                        cx.spawn(async move {
                            if let Some(gate) = gate { let _ = gate.await; }
                            connection.send_notification(SessionNotification::new(req.session_id, SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new("answer"))))))?;
                            r.respond(PromptResponse::new(StopReason::EndTurn).usage(Usage::new(15, 10, 5)))?; Ok(())
                        })?; Ok(())
                    }
                }, agent_client_protocol::on_receive_request!())
                .on_receive_notification(move |req: CancelNotification, _| {
                    let rec = cancel.clone(); async move { rec.teardown.lock().unwrap().push(format!("cancel:{}", req.session_id.0)); Ok(()) }
                }, agent_client_protocol::on_receive_notification!())
                .on_receive_request(move |req: CloseSessionRequest, r: Responder<CloseSessionResponse>, _| {
                    let rec = close.clone(); async move { rec.teardown.lock().unwrap().push(format!("close:{}", req.session_id.0)); r.respond(CloseSessionResponse::new())?; Ok(()) }
                }, agent_client_protocol::on_receive_request!())
                .connect_to(agent).await.unwrap();
        });
        let backend = AcpBackend::connect(
            client,
            AcpConfig {
                cwd: std::env::temp_dir(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        (rec, Arc::new(backend))
    }
}

#[tokio::test]
async fn fixture_fidelity() {
    use bridge_core::{domain::SessionSpec, ids::SessionId};
    use futures::StreamExt;
    let (rec, backend) = ColdAcp::connect().await;
    let key = SessionId::parse("same-key").unwrap();
    for model in ["model", "good"] {
        backend
            .configure_session(
                &key,
                &SessionSpec {
                    config: EffectiveConfig {
                        model: Some(model.into()),
                        ..Default::default()
                    },
                    cwd: None,
                },
            )
            .await
            .unwrap();
        let mut stream = backend.prompt(&key, vec![]).await.unwrap();
        while let Some(update) = stream.next().await {
            update.unwrap();
        }
        backend.forget_session(&key).await;
    }
    let prompts = rec.prompts.lock().unwrap();
    assert_eq!(rec.sessions.lock().unwrap().len(), 1);
    assert_eq!(prompts[0].0, prompts[1].0);
    assert!(prompts
        .iter()
        .all(|(_, cfg)| cfg.model.as_deref() == Some("model")));
    assert!(rec.teardown.lock().unwrap().is_empty());
}
