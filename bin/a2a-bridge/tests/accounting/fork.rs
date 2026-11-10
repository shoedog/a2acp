//! Context fork through the in-process served stack: a real `AcpBackend` over an offline adapter that
//! speaks `session/fork`, behind the real `InboundServer`, session manager and accounting.
use crate::{cold_acp_fixture::ColdAcp, support::*};
use bridge_core::{governance::*, ids::*};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;

fn owner() -> AttemptIdentity {
    AttemptIdentity::initial().unwrap()
}

fn fixture(backend: Arc<bridge_acp::acp_backend::AcpBackend>, policy: bool) -> Fixture {
    let registry = crate::common::single_agent_registry("agent", backend);
    let policy = policy.then(|| {
        Arc::new(
            BudgetPolicyV1::new(
                vec![BudgetRuleV1 {
                    id: "request".into(),
                    selector: BudgetSelectorV1::EachRequest,
                    limits: BudgetLimitsV1 {
                        turns: Some(1),
                        terminal_tokens: None,
                    },
                }],
                vec![],
            )
            .unwrap(),
        )
    });
    Fixture::with_policy(Arc::new(Recording::default()), registry, policy)
}

async fn send(
    router: axum::Router,
    method: &str,
    owner: &AttemptIdentity,
    context: Option<&str>,
    metadata: Value,
) -> Value {
    let mut md = json!({
        "a2a-bridge.execution_id": owner.execution_id.as_str(),
        "a2a-bridge.attempt_id": owner.attempt_id.as_str(),
    });
    for (key, value) in metadata.as_object().unwrap() {
        md[key] = value.clone();
    }
    let body = json!({"jsonrpc":"2.0", "id":1, "method":method, "params":{"message":{
        "text":"go", "taskId":owner.execution_id.as_str(), "contextId":context, "metadata":md}}});
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .header("A2A-Version", "1.0")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    // A streaming reply is SSE; the fork's own context rides every frame, so keep the raw text too.
    serde_json::from_str(&text).unwrap_or_else(|_| json!({"sse": text}))
}

async fn wait_for(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition was never met");
}

fn prompted(rec: &ColdAcp) -> Vec<String> {
    rec.prompts
        .lock()
        .unwrap()
        .iter()
        .map(|(s, _)| s.clone())
        .collect()
}

fn answered(reply: &Value) -> bool {
    reply.to_string().contains("answer")
}

#[tokio::test]
async fn fork_sends_one_session_fork_and_independent_turns_reach_their_own_sessions() {
    for (method, policy) in [
        ("SendMessage", false),
        ("SendStreamingMessage", false),
        ("SendMessage", true),
        ("SendStreamingMessage", true),
    ] {
        let (rec, backend) = ColdAcp::connect_forkable().await;
        let f = fixture(backend, policy);
        let router = f.router(false);
        let source = owner();
        assert!(answered(
            &send(router.clone(), method, &source, Some("src"), json!({})).await
        ));

        for (name, expected) in [("fork-a", "fork-0"), ("fork-b", "fork-1")] {
            let reply = send(
                router.clone(),
                method,
                &owner(),
                Some(name),
                json!({"a2a-bridge.fork_from": "src"}),
            )
            .await;
            assert!(answered(&reply), "{method} {policy}: {reply}");
            assert!(
                reply.to_string().contains(name),
                "the reply names the new context: {reply}"
            );
            assert_eq!(rec.forks.lock().unwrap().last().unwrap().1, expected);
        }
        assert_eq!(
            *rec.forks.lock().unwrap(),
            [
                ("mint-0".to_owned(), "fork-0".to_owned()),
                ("mint-0".to_owned(), "fork-1".to_owned())
            ],
            "exactly one session/fork per fork request, each of the source's agent session"
        );
        // The source, fork A and fork B each continue on their own session.
        for context in ["src", "fork-a", "fork-b", "src"] {
            assert!(answered(
                &send(router.clone(), method, &owner(), Some(context), json!({})).await
            ));
        }
        assert_eq!(
            prompted(&rec),
            ["mint-0", "fork-0", "fork-1", "mint-0", "fork-0", "fork-1", "mint-0"],
            "{method} policy={policy}"
        );
    }
}

#[tokio::test]
async fn two_forks_of_one_source_prompt_concurrently() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        let (rec, backend) = ColdAcp::connect_forkable().await;
        let f = fixture(backend, false);
        let router = f.router(false);
        assert!(answered(
            &send(router.clone(), method, &owner(), Some("src"), json!({})).await
        ));

        // Both forks' replies are held until both prompts have reached the adapter.
        let (open_a, gate_a) = tokio::sync::oneshot::channel();
        let (open_b, gate_b) = tokio::sync::oneshot::channel();
        *rec.gates.lock().unwrap() = vec![None, Some(gate_a), Some(gate_b)];
        let requests: Vec<_> = ["fork-a", "fork-b"]
            .into_iter()
            .map(|name| {
                let router = router.clone();
                tokio::spawn(async move {
                    send(
                        router,
                        method,
                        &owner(),
                        Some(name),
                        json!({"a2a-bridge.fork_from": "src"}),
                    )
                    .await
                })
            })
            .collect();
        wait_for(|| rec.prompts.lock().unwrap().len() == 3).await;
        let mut sessions = prompted(&rec)[1..].to_vec();
        sessions.sort();
        assert_eq!(sessions, ["fork-0", "fork-1"]);
        open_a.send(()).unwrap();
        open_b.send(()).unwrap();
        for request in requests {
            assert!(answered(&request.await.unwrap()));
        }
    }
}

#[tokio::test]
async fn a_forked_turn_is_accounted_to_its_own_execution_agent_and_selection() {
    let (_rec, backend) = ColdAcp::connect_forkable().await;
    let f = fixture(backend, false);
    let router = f.router(false);
    let (first, second) = (owner(), owner());
    send(
        router.clone(),
        "SendMessage",
        &first,
        Some("src"),
        json!({}),
    )
    .await;
    let reply = send(
        router,
        "SendMessage",
        &second,
        Some("fork"),
        json!({"a2a-bridge.fork_from": "src", "a2a-bridge.model": "good"}),
    )
    .await;
    assert!(answered(&reply), "{reply}");
    let rows = f.rows();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let fork = rows
        .iter()
        .find(|row| row.attribution.execution.as_deref() == Some(second.execution_id.as_str()))
        .expect("the fork has a row of its own");
    assert_eq!(fork.attribution.agent, "agent");
    assert_eq!(fork.attribution.effective_model.as_deref(), Some("good"));
    assert!(fork.finalized && fork.dispatched);
    assert_billing(fork);
    assert_eq!(fork.usage.terminal.as_ref().unwrap().total_tokens, 15);
}

#[tokio::test]
async fn cancelling_a_forks_turn_does_not_cancel_the_source() {
    let (rec, backend) = ColdAcp::connect_forkable().await;
    let f = fixture(backend, false);
    let router = f.router(false);
    send(
        router.clone(),
        "SendMessage",
        &owner(),
        Some("src"),
        json!({}),
    )
    .await;

    let (finish, gate) = tokio::sync::oneshot::channel();
    *rec.gates.lock().unwrap() = vec![None, Some(gate)];
    let identity = owner();
    let running = tokio::spawn({
        let (router, identity) = (router.clone(), identity.clone());
        async move {
            send(
                router,
                "SendMessage",
                &identity,
                Some("fork"),
                json!({"a2a-bridge.fork_from": "src"}),
            )
            .await
        }
    });
    wait_for(|| rec.prompts.lock().unwrap().len() == 2).await;
    let cancelled = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .header("A2A-Version", "1.0")
                .body(axum::body::Body::from(
                    json!({"jsonrpc":"2.0","id":1,"method":"CancelTask",
                           "params":{"id": identity.execution_id.as_str()}})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let cancelled = axum::body::to_bytes(cancelled.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&cancelled).contains("CANCELED"));
    wait_for(|| !rec.teardown.lock().unwrap().is_empty()).await;
    assert_eq!(*rec.teardown.lock().unwrap(), ["cancel:fork-0"]);
    finish.send(()).unwrap();
    let _ = running.await;

    // The source was never cancelled and still takes its own turn.
    assert!(answered(
        &send(router, "SendMessage", &owner(), Some("src"), json!({})).await
    ));
    assert_eq!(prompted(&rec).last().unwrap(), "mint-0");
    assert_eq!(*rec.teardown.lock().unwrap(), ["cancel:fork-0"]);
}

#[tokio::test]
async fn a_refused_fork_sends_nothing_and_charges_nothing() {
    for policy in [false, true] {
        let (rec, backend) = ColdAcp::connect_forkable().await;
        let f = fixture(backend, policy);
        let router = f.router(false);
        send(
            router.clone(),
            "SendMessage",
            &owner(),
            Some("src"),
            json!({}),
        )
        .await;
        let rows = f.rows().len();
        let prompts = rec.prompts.lock().unwrap().len();

        let refused = |reply: &Value, reason: &str| {
            assert_eq!(
                reply["error"]["data"]["reason"], reason,
                "policy={policy}: {reply}"
            );
        };
        let reply = send(
            router.clone(),
            "SendMessage",
            &owner(),
            Some("t"),
            json!({"a2a-bridge.fork_from": "nobody"}),
        )
        .await;
        refused(&reply, "fork_source_not_found");
        let reply = send(
            router.clone(),
            "SendMessage",
            &owner(),
            Some("src"),
            json!({"a2a-bridge.fork_from": "src"}),
        )
        .await;
        refused(&reply, "fork_target_in_use");

        assert!(rec.forks.lock().unwrap().is_empty());
        assert_eq!(rec.prompts.lock().unwrap().len(), prompts);
        let after = f.rows();
        assert!(
            after[rows..]
                .iter()
                .all(|row| !row.dispatched && row.usage.terminal.is_none()),
            "a refused fork's accounting never reached a provider: {after:?}"
        );
        // The refused source is untouched and forks afterwards.
        let reply = send(
            router,
            "SendMessage",
            &owner(),
            Some("t"),
            json!({"a2a-bridge.fork_from": "src"}),
        )
        .await;
        assert!(answered(&reply), "{reply}");
    }
}

#[tokio::test]
async fn an_adapter_without_the_capability_is_refused_before_any_fork() {
    let (rec, backend) = ColdAcp::connect().await;
    let f = fixture(backend, false);
    let router = f.router(false);
    send(
        router.clone(),
        "SendMessage",
        &owner(),
        Some("src"),
        json!({}),
    )
    .await;
    let prompts = rec.prompts.lock().unwrap().len();
    let reply = send(
        router,
        "SendMessage",
        &owner(),
        Some("t"),
        json!({"a2a-bridge.fork_from": "src"}),
    )
    .await;
    assert_eq!(
        reply["error"]["data"]["reason"], "fork_adapter_unsupported",
        "{reply}"
    );
    assert!(rec.forks.lock().unwrap().is_empty());
    assert_eq!(rec.prompts.lock().unwrap().len(), prompts);
}
