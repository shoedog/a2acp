//! Provider-free CLI activation through production ACP construction, plus remote isolation.
#![cfg(unix)]

use bridge_core::{accounting::*, ids::*, task_store::TaskStore};
use bridge_store::sqlite::SqliteStore;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command, sync::Arc};
mod common;
#[allow(dead_code)]
#[path = "accounting/support.rs"]
mod support;
mod text_file_busy;

struct Cli {
    dir: tempfile::TempDir,
    config: PathBuf,
    prompts: PathBuf,
    store: PathBuf,
    platform: PathBuf,
}
impl Cli {
    fn new(configured: bool, telemetry: bool, model: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let adapter = dir.path().join("fixture-acp");
        let prompts = dir.path().join("prompts.jsonl");
        // Observe the exact delivered prompt independently of accounting. Both usage sources
        // travel through the real ACP mapper: occupancy/cost notification, terminal token totals.
        fs::write(&adapter, r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -E 's/^\{"jsonrpc":"2.0","id":([^,]+),"method".*/\1/')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]},"id":%s}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","result":{"sessionId":"fixture","models":{"availableModels":[{"modelId":"good","name":"Good"}],"currentModelId":"good"}},"id":%s}\n' "$id" ;;
    *'"method":"session/set_model"'*|*'"method":"session/set_mode"'*|*'"method":"session/set_config_option"'*) printf '{"jsonrpc":"2.0","result":{},"id":%s}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' "$line" >> "$1"
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fixture","update":{"sessionUpdate":"usage_update","used":99,"size":1000,"cost":{"amount":0.25,"currency":"USD"}}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fixture","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"answer"}}}}'
      printf '{"jsonrpc":"2.0","result":{"stopReason":"end_turn","usage":{"totalTokens":15,"inputTokens":10,"outputTokens":5}},"id":%s}\n' "$id" ;;
  esac
done
"#).unwrap();
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
        text_file_busy::warm_up_script(&adapter);
        fs::write(
            dir.path().join("input.md"),
            "---\ntask-type: freeform\n---\nbrief",
        )
        .unwrap();
        let config = dir.path().join("a2a-bridge.toml");
        let store_config = if configured {
            "[store]\npath = \"tasks.sqlite\"\n"
        } else {
            ""
        };
        let billing_config = if configured {
            r#"[[billing_identities]]
agent = "agent"
model = "good"
account_group = "fixture"
billing_mode = "metered"
"#
        } else {
            ""
        };
        fs::write(
            &config,
            format!(
                r#"
default = "agent"
{store_config}
{billing_config}
[registry]
allowed_cmds = [{adapter:?}]
[metrics]
enabled = {telemetry}
[traces]
enabled = {telemetry}
[[agents]]
id = "agent"
cmd = {adapter:?}
args = [{prompts:?}]
pre_authenticated = true
[[workflows]]
id = "flow"
[[workflows.nodes]]
id = "review"
agent = "agent"
model = {model:?}
prompt_text = "Review: {{{{input}}}}"
inputs = []
[server]
addr = "127.0.0.1:0"
"#
            ),
        )
        .unwrap();
        let platform = dir.path().join("platform");
        fs::create_dir(&platform).unwrap();
        fs::set_permissions(&platform, fs::Permissions::from_mode(0o700)).unwrap();
        let store = if configured {
            dir.path().join("tasks.sqlite")
        } else {
            platform.join("workflow-history.sqlite")
        };
        Self {
            dir,
            config,
            prompts,
            store,
            platform,
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
        cmd.args([
            "run-workflow",
            "flow",
            "--no-heartbeat",
            "--input",
            "input.md",
            "--config",
        ])
        .arg(&self.config)
        .current_dir(self.dir.path())
        .env("A2A_BRIDGE_STATE_DIR", &self.platform);
        cmd
    }
    fn prompts(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(&self.prompts)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn rows(&self) -> Vec<UsageAttemptV1> {
        let path = self.store.with_extension("sqlite.usage-v1.sqlite");
        if !path.exists() {
            return vec![];
        }
        let conn =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let rows = conn
            .prepare("SELECT record_json FROM usage_attempts_v1")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect();
        rows
    }
    async fn assert_accounted(&self, output: &std::process::Output) {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("answer"));
        let prompts = self.prompts();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0]["params"]["prompt"][0]["text"], "Review: brief");
        let rows = self.rows();
        assert_eq!(
            rows.len(),
            prompts.len(),
            "every actual CLI prompt must be accounted"
        );
        let (execution, attempt) = locator(output);
        let row = &rows[0];
        assert_eq!(
            row.attribution.billing.is_some(),
            self.store.file_name().unwrap() == "tasks.sqlite"
        );
        if row.attribution.billing.is_some() {
            support::assert_billing(row);
        }
        assert_eq!(
            row.attribution.execution.as_deref(),
            Some(execution.as_str())
        );
        assert_eq!(
            row.attribution.run_attempt.as_deref(),
            Some(attempt.as_str())
        );
        assert_eq!(row.attribution.turn.as_ref(), Some(&row.id));
        assert_eq!(row.attribution.workflow.as_deref(), Some("flow"));
        assert_eq!(row.attribution.node.as_deref(), Some("review"));
        assert_eq!(row.attribution.agent, "agent");
        assert_eq!(row.attribution.effective_model.as_deref(), Some("good"));
        assert_eq!(row.attribution.attempt_kind, "node");
        assert!(row.dispatched && row.finalized);
        let reopened = SqliteStore::open_history_read_only(&self.store).unwrap();
        let summary = reopened
            .usage_summary_v1(&UsageQueryV1 {
                execution: Some(execution),
                run_attempt: Some(attempt),
                workflow: Some("flow".into()),
                node: Some("review".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(summary.buckets.len(), 1);
        let bucket = &summary.buckets[0];
        assert_eq!(
            (bucket.attempted, bucket.dispatched, bucket.pending),
            (1, 1, 0)
        );
        assert_eq!(
            (
                bucket.total_tokens.reported_sum,
                bucket.input_tokens.reported_sum,
                bucket.output_tokens.reported_sum
            ),
            (15, 10, 5)
        );
        assert_eq!(bucket.cost_by_currency["USD"], 0.25);
    }
}
fn locator(output: &std::process::Output) -> (String, String) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let get = |key: &str| {
        stdout
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .unwrap()
            .to_string()
    };
    (get("execution_id="), get("attempt_id="))
}

#[tokio::test]
async fn configured_store_reopens_exact_cli_usage_with_and_without_telemetry() {
    for telemetry in [true, false] {
        let cli = Cli::new(true, telemetry, "good");
        let output = cli.command().output().unwrap();
        cli.assert_accounted(&output).await;
        assert!(
            !cli.platform.join("workflow-history.sqlite").exists(),
            "configured allocation must be the only destination"
        );
    }
}

#[tokio::test]
async fn platform_allocation_is_the_only_unconfigured_accounting_destination() {
    let cli = Cli::new(false, false, "good");
    let output = cli.command().output().unwrap();
    cli.assert_accounted(&output).await;
    assert!(!cli.dir.path().join("tasks.sqlite").exists());
    assert_eq!(
        fs::read_dir(&cli.platform)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry
                .file_name()
                .to_string_lossy()
                .ends_with(".usage-v1.sqlite"))
            .count(),
        1
    );
}

#[tokio::test]
async fn optional_history_reservation_failure_keeps_accounting_active() {
    let cli = Cli::new(true, false, "good");
    drop(SqliteStore::open_shared_history(&cli.store).unwrap());
    let conn = rusqlite::Connection::open(&cli.store).unwrap();
    conn.execute_batch("CREATE TRIGGER refuse_history BEFORE INSERT ON workflow_history_attachment BEGIN SELECT RAISE(ABORT, 'telemetry refused'); END;").unwrap();
    drop(conn);
    let output = cli.command().output().unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("telemetry_unavailable"));
    cli.assert_accounted(&output).await;
}

async fn assert_refused(failure: &str) {
    let cli = Cli::new(failure != "platform-allocation", false, "good");
    match failure {
        "allocation" | "platform-allocation" => fs::create_dir(&cli.store).unwrap(),
        "insert" => {
            let store = SqliteStore::open_shared_history(&cli.store).unwrap();
            store.usage_summary_v1(&Default::default()).await.unwrap();
            drop(store);
            let conn =
                rusqlite::Connection::open(cli.store.with_extension("sqlite.usage-v1.sqlite"))
                    .unwrap();
            conn.execute_batch("CREATE TRIGGER refuse_usage BEFORE INSERT ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'accounting refused'); END;").unwrap();
        }
        // The frozen graph is admitted, but resolving the selected backend cannot spawn it.
        // ACP model negotiation happens lazily at prompt-open and is not preparation.
        "preparation" => fs::remove_file(cli.dir.path().join("fixture-acp")).unwrap(),
        _ => unreachable!(),
    }
    let output = cli.command().output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "{failure}: workflow must fail closed"
    );
    assert!(
        cli.prompts().is_empty(),
        "{failure}: refusal must precede provider prompt"
    );
    let rows = cli.rows();
    assert_eq!(rows.len(), usize::from(failure == "preparation"));
    if let Some(row) = rows.first() {
        assert!(
            !row.dispatched && row.finalized,
            "admitted rejection must settle: {row:?}; {stderr}"
        );
        let (execution, attempt) = locator(&output);
        assert_eq!(
            row.attribution.execution.as_deref(),
            Some(execution.as_str())
        );
        assert_eq!(
            row.attribution.run_attempt.as_deref(),
            Some(attempt.as_str())
        );
    }
}
#[tokio::test]
async fn unavailable_allocations_refuse_every_prompt() {
    for failure in ["allocation", "platform-allocation"] {
        assert_refused(failure).await;
    }
}
#[tokio::test]
async fn accounting_insert_refusal_prevents_every_prompt() {
    assert_refused("insert").await;
}
#[tokio::test]
async fn admitted_preparation_failure_leaves_an_undispatched_finalized_row() {
    assert_refused("preparation").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_client_leaves_accounting_and_prompt_ownership_to_executing_service() {
    use bridge_core::{
        domain::{RouteTarget, TaskMeta},
        error::BridgeError,
        ports::RouteDecision,
    };
    struct Route;
    impl RouteDecision for Route {
        fn route(&self, _: &TaskMeta) -> Result<RouteTarget, BridgeError> {
            Ok(RouteTarget::Workflow(WorkflowId::parse("flow").unwrap()))
        }
    }
    let f = support::Fixture::new();
    let registry = common::single_agent_registry("agent", f.backend.clone());
    let graph = Arc::new(
        serde_json::from_value(serde_json::json!({"id":"flow", "nodes":[
            {"id":"review", "agent":"agent", "prompt_template":"{{input}}", "inputs":[]}
        ]}))
        .unwrap(),
    );
    let coordinator = Arc::new(
        bridge_coordinator::Coordinator::new(
            Arc::new(bridge_coordinator::session_manager::SessionManager::new(
                registry.clone(),
                std::time::Duration::from_secs(60),
            )),
            Some(Arc::new(bridge_workflow::executor::WorkflowExecutor::new(
                registry.clone(),
            ))),
            Arc::new([(WorkflowId::parse("flow").unwrap(), graph)].into()),
            f.store.clone(),
            f.store.clone(),
            Arc::new(bridge_policy::permission::AutoPolicy),
            registry,
            Arc::new(bridge_coordinator::clock::SystemClock),
            None,
            None,
            Arc::new(bridge_observ::NoopObserver),
            3,
        )
        .with_accounting(support::billing_accounting(f.store.clone())),
    );
    let router = Arc::new(bridge_a2a_inbound::server::InboundServer::from_coordinator(
        coordinator.clone(),
        Arc::new(Route),
        Arc::new(bridge_policy::auth::AlwaysGrant),
        "http://localhost",
        Arc::new(bridge_a2a_outbound::StubDelegation),
        "agent",
    ))
    .router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let cli = Cli::new(true, false, "good");
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args([
            "run-workflow",
            "flow",
            "--input",
            "input.md",
            "--no-heartbeat",
            "--serve",
            "--context",
            "cli-context",
            "--url",
            &url,
        ])
        .current_dir(cli.dir.path())
        .env("A2A_BRIDGE_STATE_DIR", &cli.platform)
        .output()
        .unwrap();
    server.abort();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(f.backend.prompts.lock().unwrap().len(), 1);
    assert!(cli.prompts().is_empty());
    assert!(
        !cli.store.exists(),
        "remote client must not open its configured store"
    );
    assert!(!cli.platform.join("workflow-history.sqlite").exists());
    assert!(cli.rows().is_empty());
    let (execution, attempt) = locator(&output);
    let owner = AttemptIdentity {
        execution_id: ExecutionId::parse(execution).unwrap(),
        attempt_id: AttemptId::parse(attempt).unwrap(),
        ordinal: 0,
        parent_attempt_id: None,
    };
    support::assert_usage(&f.reopened(Some(&owner)).await, 1, 0);
    assert_eq!(f.rows().len(), 1, "only the executing service may account");
}
