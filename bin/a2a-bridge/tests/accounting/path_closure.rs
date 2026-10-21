//! G-A2.2d3: close row/owner assertions without expanding the construction matrix.
//!
//! Retained entry-point witnesses (test target :: test name):
//! - accounting_production::direct::served_cold_warm_unary_streaming_and_fanout_reopen_exact_actuals
//!   covers unary/streaming cold/warm and both fan-out local legs.
//! - accounting_production::direct::review_mcp_transport_run_and_continue_share_session_not_accounting_owner
//!   covers MCP run/continue; compaction_usage_has_its_own_scope_and_retained_config covers explicit compact.
//! - accounting_workflow_cli::configured_store_reopens_exact_cli_usage_with_and_without_telemetry
//!   covers local CLI; serve_client_leaves_accounting_and_prompt_ownership_to_executing_service
//!   covers the remote client's absence of local charges.
//! - accounting_workflow_served::served_rpc_reopens_supplied_identity_without_optional_telemetry
//!   covers served workflow.
//! - accounting_workflow_batch::batch_child_filters_and_resumed_checkpoint_ownership covers fresh
//!   and resumed children; non_batch_recovery_forwards_owner_with_and_without_batch_runtime covers boot recovery.
//! - accounting_implement::fresh_edit_review_fix_approval_is_durable covers edit/fix/review;
//!   resumed_cli_matches_each_prompt_to_exact_reported_usage below covers resume. The binary unit test
//!   production_implement_accounting_keeps_typed_failures_and_rebuild_roles retains helper-level rebuild coverage;
//!   eligible_rebuild_cli_preserves_owner_and_exact_provider_usage below enters the CLI itself.
//! - accounting_smoke::standalone_smoke_reopens_exact_usage_under_artifact_owner covers smoke.
//! - accounting_compatibility::staged_child_usage_survives_cleanup_and_aggregate_rereads covers the actual child.
//!
//! These fixtures supply undecorated providers to production callers. Accounting activation and capture
//! remain production responsibilities. Earlier contract tests and broader combinations stay enabled.

use crate::{direct::request, support::*};
use bridge_core::{accounting::*, ids::*};
use std::{collections::HashSet, sync::Arc};

async fn exact_rows(f: &Fixture, owners: &[AttemptIdentity]) -> Vec<UsageAttemptV1> {
    let rows = f.rows(); // A new SQLite connection, independent of the writer.
    assert_eq!(f.backend.prompts.lock().unwrap().len(), owners.len());
    assert_eq!(
        rows.len(),
        owners.len(),
        "one row per inner prompt, no extras"
    );
    assert_eq!(
        rows.iter().map(|r| &r.id).collect::<HashSet<_>>().len(),
        rows.len()
    );
    for owner in owners {
        let matching: Vec<_> = rows
            .iter()
            .filter(|r| {
                r.attribution.execution.as_deref() == Some(owner.execution_id.as_str())
                    && r.attribution.run_attempt.as_deref() == Some(owner.attempt_id.as_str())
            })
            .collect();
        assert_eq!(matching.len(), 1, "{owner:?}: {rows:?}");
        let row = matching[0];
        assert_eq!(row.attribution.turn.as_ref(), Some(&row.id));
        assert_ne!(row.id, owner.attempt_id.as_str());
        assert_eq!(row.attribution.agent, "agent");
        assert!(row.dispatched && row.finalized);
        assert_eq!(row.usage, usage());
        assert!(!row.cost_invalid);
        assert_usage(&f.reopened(Some(owner)).await, 1, 0);
    }
    assert_usage(&f.reopened(None).await, owners.len() as u64, 0);
    rows
}

#[tokio::test]
async fn served_unary_and_streaming_cold_warm_have_bijective_ownership() {
    let f = Fixture::new();
    let mut owners = Vec::new();
    // One cold and one warm turn per transport, sharing a backend but retaining distinct owners.
    for (method, context) in [
        ("SendMessage", "unary"),
        ("SendStreamingMessage", "streaming"),
    ] {
        for _ in 0..2 {
            let owner = AttemptIdentity::initial().unwrap();
            let body = request(f.router(false), method, &owner, Some(context)).await;
            assert!(body.contains("answer"), "{body}");
            owners.push(owner);
        }
    }
    let prompts = f.backend.prompts.lock().unwrap().clone();
    assert_eq!(prompts[0], prompts[1]);
    assert_eq!(prompts[2], prompts[3]);
    assert_ne!(prompts[0], prompts[2]);
    for row in exact_rows(&f, &owners).await {
        assert_eq!(row.attribution.attempt_kind, "direct");
    }
}

struct Peer;
#[async_trait::async_trait]
impl bridge_core::ports::DelegationPort for Peer {
    async fn delegate(
        &self,
        _: &bridge_core::domain::AuthContext,
        _: &TaskId,
        _: Vec<bridge_core::domain::Part>,
    ) -> Result<bridge_core::ports::Delegation, bridge_core::error::BridgeError> {
        let mut remote = usage();
        remote.terminal.as_mut().unwrap().total_tokens = 900;
        remote.cost.as_mut().unwrap().amount = 99.0;
        Ok(bridge_core::ports::Delegation {
            events: Box::pin(futures::stream::iter([
                Ok(bridge_core::translator::Event::usage(remote)),
                Ok(bridge_core::translator::Event::artifact("peer answer")),
            ])),
            peer_task: tokio::sync::watch::channel(None).1,
        })
    }
    async fn cancel(
        &self,
        _: &bridge_core::domain::PeerTaskId,
    ) -> Result<(), bridge_core::error::BridgeError> {
        Ok(())
    }
}

#[tokio::test]
async fn unary_and_streaming_fanout_charge_only_the_local_prompt() {
    struct Fanout;
    impl bridge_core::ports::RouteDecision for Fanout {
        fn route_before_default(
            &self,
            meta: &bridge_core::domain::TaskMeta,
        ) -> Result<Option<bridge_core::domain::RouteTarget>, bridge_core::error::BridgeError>
        {
            self.route(meta).map(Some)
        }
        fn route(
            &self,
            _: &bridge_core::domain::TaskMeta,
        ) -> Result<bridge_core::domain::RouteTarget, bridge_core::error::BridgeError> {
            Ok(bridge_core::domain::RouteTarget::Fanout)
        }
    }
    let f = Fixture::new();
    let router = Arc::new(bridge_a2a_inbound::server::InboundServer::from_coordinator(
        f.coordinator.clone(),
        Arc::new(Fanout),
        Arc::new(bridge_policy::auth::AlwaysGrant),
        "http://localhost",
        Arc::new(Peer),
        "agent",
    ))
    .router();
    let mut owners = Vec::new();
    for method in ["SendMessage", "SendStreamingMessage"] {
        let owner = AttemptIdentity::initial().unwrap();
        let body = request(router.clone(), method, &owner, None).await;
        assert!(body.contains("peer answer"), "{body}");
        assert!(body.contains("\"answer\""), "{body}");
        owners.push(owner);
    }
    for row in exact_rows(&f, &owners).await {
        assert_eq!(row.attribution.node.as_deref(), Some("agent"));
    }
}

#[tokio::test]
async fn mcp_transport_and_explicit_compaction_leave_no_unowned_or_duplicate_rows() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let f = Fixture::new();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (read, write) = tokio::io::split(server);
    let coordinator = f.coordinator.clone();
    let serving =
        tokio::spawn(async move { bridge_mcp::serve(read, write, coordinator).await.unwrap() });
    let (read, mut write) = tokio::io::split(client);
    let mut lines = tokio::io::BufReader::new(read).lines();
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await.unwrap();
    lines.next_line().await.unwrap().unwrap();
    let mut owners = Vec::new();
    for name in ["run", "continue"] {
        let owner = AttemptIdentity::initial().unwrap();
        let req = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":name,"arguments":{"input":"go","context":"mcp-closure",
                "execution_id":owner.execution_id.as_str(),"attempt_id":owner.attempt_id.as_str()}}});
        write
            .write_all(format!("{req}\n").as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(response["result"].get("isError").is_none(), "{line}");
        assert!(line.contains("answer"), "{line}");
        owners.push(owner);
    }
    exact_rows(&f, &owners).await;
    let before = f.rows();
    use tower::ServiceExt;
    let response = f
        .router(false)
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .header("A2A-Version", "1.0")
                .body(axum::body::Body::from(
                    serde_json::json!({"jsonrpc":"2.0","id":3,"method":"SessionCompact",
            "params":{"contextId":"mcp-closure"}})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("compacted"),
        "{body:?}"
    );
    let rows = f.rows();
    let compact = rows
        .iter()
        .find(|r| r.attribution.attempt_kind == "compaction")
        .unwrap();
    let compact_owner = AttemptIdentity {
        execution_id: ExecutionId::parse(compact.attribution.execution.clone().unwrap()).unwrap(),
        attempt_id: AttemptId::parse(compact.attribution.run_attempt.clone().unwrap()).unwrap(),
        ordinal: 0,
        parent_attempt_id: None,
    };
    assert!(owners
        .iter()
        .all(|o| o.execution_id != compact_owner.execution_id));
    owners.push(compact_owner);
    exact_rows(&f, &owners).await;
    for old in before {
        assert_eq!(rows.iter().find(|r| r.id == old.id), Some(&old));
    }
    {
        let prompts = f.backend.prompts.lock().unwrap();
        assert!(prompts.iter().all(|id| id == &prompts[0]));
    }
    write.shutdown().await.unwrap();
    drop(write);
    serving.await.unwrap();
}

#[cfg(unix)]
#[path = "implement_support.rs"]
mod implement_support;

#[cfg(unix)]
#[test]
fn resumed_cli_matches_each_prompt_to_exact_reported_usage() {
    use implement_support::{success, Fixture};
    let f = Fixture::new(1);
    let runtime = f.dir.path().join("runtime");
    let provider = std::fs::read_to_string(&runtime).unwrap();
    let response = "    print(json.dumps({'jsonrpc':'2.0','id':ident,'result':result}),flush=True)";
    assert_eq!(provider.matches(response).count(), 1);
    let provider = provider.replace(
        response,
        &format!(
            r#"    if method == 'session/prompt':
        n = len((root/'prompts').read_text().splitlines())
        result['usage'] = {{'totalTokens':120+2*n,'inputTokens':100+n,'outputTokens':20+n}}
        with (root/'reported-usage').open('a') as f:
            f.write(json.dumps({{'text':text,'usage':result['usage']}})+'\n')
{response}"#
        ),
    );
    std::fs::write(runtime, provider).unwrap();
    success(&f.run(false, ""));
    let previous = f.rows();
    let previous_prompts = f.prompts();
    assert_eq!(previous.len(), 2);
    assert_eq!(previous_prompts.len(), 2);
    let clone = f.clone_path();
    let old = bridge_controller::implement_resume::load_accounting_owner(&clone)
        .unwrap()
        .unwrap();
    // Model an interrupted in-loop checkpoint, retaining the durable earlier rows.
    f.strand();
    std::fs::write(f.dir.path().join("reviews"), "0").unwrap();
    success(&f.run(true, ""));
    let owner = bridge_controller::implement_resume::load_accounting_owner(&clone)
        .unwrap()
        .unwrap();
    assert_eq!(owner.execution_id, old.execution_id);
    assert_ne!(owner.attempt_id, old.attempt_id);
    let rows = f.rows(); // Reopen SQLite after the resume process exits.
    let prompts = f.prompts();
    let reported: Vec<serde_json::Value> =
        std::fs::read_to_string(f.dir.path().join("reported-usage"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert_eq!(rows.len(), 5);
    assert_eq!(prompts.len(), 5);
    assert_eq!(reported.len(), prompts.len());
    assert_eq!(rows[..previous.len()], previous);
    assert_eq!(prompts[..previous_prompts.len()], previous_prompts);
    assert_eq!(
        rows.iter().map(|r| &r.id).collect::<HashSet<_>>().len(),
        rows.len()
    );
    let resumed = &rows[previous.len()..];
    // Resume prepares the fix row before its initial review. Dispatch order is review,
    // fix, review; associate observations independently of the usage values under test.
    let prompt_rows = [&resumed[1], &resumed[0], &resumed[2]];
    for (index, ((row, prompt), report)) in prompt_rows
        .into_iter()
        .zip(&prompts[2..])
        .zip(&reported[2..])
        .enumerate()
    {
        assert_eq!(report["text"], prompt["text"]);
        let n = index as u64 + 3;
        assert_eq!(
            report["usage"],
            serde_json::json!({
                "totalTokens":120+2*n,"inputTokens":100+n,"outputTokens":20+n
            })
        );
        assert_eq!(
            row.attribution.execution.as_deref(),
            Some(owner.execution_id.as_str())
        );
        assert_eq!(
            row.attribution.run_attempt.as_deref(),
            Some(owner.attempt_id.as_str())
        );
        assert_eq!(row.attribution.turn.as_ref(), Some(&row.id));
        let review = prompt["text"].as_str().unwrap().starts_with("REVIEW");
        assert_eq!(
            row.attribution.agent,
            if review { "reviewer" } else { "writer" }
        );
        assert_eq!(
            row.attribution.effective_model.as_deref(),
            prompt["config"]["model"].as_str()
        );
        assert_eq!(
            row.attribution.effective_effort.as_deref(),
            prompt["config"]["effort"].as_str()
        );
        assert_eq!(
            row.attribution.effective_mode.as_deref(),
            prompt["config"]["mode"].as_str()
        );
        assert!(row.dispatched && row.finalized);
        let usage = row.usage.terminal.as_ref().unwrap();
        assert_eq!(report["usage"]["totalTokens"], usage.total_tokens);
        assert_eq!(report["usage"]["inputTokens"], usage.input_tokens);
        assert_eq!(report["usage"]["outputTokens"], usage.output_tokens);
    }
}

#[cfg(unix)]
#[test]
fn eligible_rebuild_cli_preserves_owner_and_exact_provider_usage() {
    use implement_support::{success, Fixture};
    for reject_replacement in [false, true] {
        let f = Fixture::new(3);
        let runtime = f.dir.path().join("runtime");
        let provider = std::fs::read_to_string(&runtime).unwrap();
        // A single real ACP session/new refusal is transient and excludes prompt acceptance.
        // Observe it outside accounting, then let the production runner rebuild exactly once.
        let refusal = format!(
            r#"
        if args[0] == 'run' and not (root/'refused').exists():
            (root/'refused').write_text('session/new')
            if {reject}: (root/'mode').write_text('replacement_reject')
            print(json.dumps({{'jsonrpc':'2.0','id':ident,'error':{{'code':-32603,'message':'fixture session refused'}}}}),flush=True)
            continue
"#,
            reject = if reject_replacement { "True" } else { "False" }
        );
        let changed = provider.replace(
            "elif method == 'session/new':",
            &format!("elif method == 'session/new':{refusal}"),
        );
        assert_ne!(provider, changed);
        let response =
            "    print(json.dumps({'jsonrpc':'2.0','id':ident,'result':result}),flush=True)";
        assert_eq!(changed.matches(response).count(), 1);
        let changed = changed.replace(response, &format!(
            "    if method == 'session/prompt':\n        with (root/'reported-usage').open('a') as f: f.write(json.dumps(result['usage'])+'\\n')\n{response}"
        ));
        std::fs::write(runtime, changed).unwrap();
        let out = f.run(false, "");
        if !reject_replacement {
            success(&out);
        }
        let rows = f.rows();
        let owner = bridge_controller::implement_resume::load_accounting_owner(&f.clone_path())
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("refused")).unwrap(),
            "session/new"
        );
        assert_eq!(
            std::fs::read_to_string(f.dir.path().join("runs"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        assert_eq!(rows.len(), if reject_replacement { 2 } else { 5 });
        assert_eq!(
            rows.iter().map(|r| &r.id).collect::<HashSet<_>>().len(),
            rows.len()
        );
        for row in &rows {
            assert_eq!(
                row.attribution.execution.as_deref(),
                Some(owner.execution_id.as_str())
            );
            assert_eq!(
                row.attribution.run_attempt.as_deref(),
                Some(owner.attempt_id.as_str())
            );
            assert_eq!(row.attribution.turn.as_ref(), Some(&row.id));
            assert!(row.dispatched);
        }
        assert_eq!(rows[0].attribution.attempt_kind, "implement_edit");
        assert!(!rows[0].finalized);
        assert_eq!(rows[0].usage, Default::default()); // Lazy first-prompt setup contract.
        assert_eq!(rows[1].attribution.attempt_kind, "implement_edit_rebuild");
        if reject_replacement {
            assert!(!out.status.success());
            assert!(f.prompts().is_empty());
            assert!(!rows[1].finalized);
            assert_eq!(rows[1].usage, Default::default());
        } else {
            let prompts = f.prompts();
            let reported = std::fs::read_to_string(f.dir.path().join("reported-usage")).unwrap();
            let reported: Vec<serde_json::Value> = reported
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(prompts.len(), 4);
            assert_eq!(reported.len(), prompts.len());
            assert_eq!(rows[3].attribution.attempt_kind, "implement_fix");
            for ((row, prompt), reported) in rows[1..].iter().zip(prompts).zip(reported) {
                assert_eq!(
                    row.attribution.agent,
                    if prompt["text"].as_str().unwrap().starts_with("REVIEW") {
                        "reviewer"
                    } else {
                        "writer"
                    }
                );
                assert_eq!(
                    row.attribution.effective_model.as_deref(),
                    prompt["config"]["model"].as_str()
                );
                assert_eq!(
                    row.attribution.effective_effort.as_deref(),
                    prompt["config"]["effort"].as_str()
                );
                assert_eq!(
                    row.attribution.effective_mode.as_deref(),
                    prompt["config"]["mode"].as_str()
                );
                assert!(row.finalized);
                let usage = row.usage.terminal.as_ref().unwrap();
                assert_eq!(reported["totalTokens"], usage.total_tokens);
                assert_eq!(reported["inputTokens"], usage.input_tokens);
                assert_eq!(reported["outputTokens"], usage.output_tokens);
            }
        }
    }
}
