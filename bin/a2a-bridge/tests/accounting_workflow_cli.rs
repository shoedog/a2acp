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
#[path = "accounting/uncapped_baseline.rs"]
mod uncapped_baseline;

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
        let mut cmd = Command::new(
            std::env::var_os("A2A_GMIN4_TEST_BINARY")
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_a2a-bridge").into()),
        );
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
        .env("A2A_BRIDGE_STATE_DIR", &self.platform)
        .env_remove("RUST_LOG");
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
async fn acp_alias_selector_keeps_its_exact_billing_mapping() {
    let cli = Cli::new(true, false, "opus");
    let adapter = cli.dir.path().join("fixture-acp");
    // ACP resolves opus to the advertised default; accounting keys the dispatched selector.
    let script = fs::read_to_string(&adapter)
        .unwrap()
        .replace("good", "default");
    fs::write(&adapter, script).unwrap();
    text_file_busy::warm_up_script(&adapter);
    let config = fs::read_to_string(&cli.config)
        .unwrap()
        .replace("model = \"good\"", "model = \"opus\"");
    fs::write(&cli.config, config).unwrap();
    let output = cli.command().output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        cli.prompts().len(),
        1,
        "billing must add no provider prompts"
    );
    let rows = cli.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].attribution.effective_model.as_deref(), Some("opus"));
    support::assert_billing(&rows[0]);
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

fn caps(cli: &Cli, request: u64, workflow: u64, tokens: bool) {
    use std::io::Write;
    let mut cfg = fs::OpenOptions::new()
        .append(true)
        .open(&cli.config)
        .unwrap();
    for (id, selector, cap) in [
        ("request", "each_request", request),
        ("workflow", "each_workflow_run", workflow),
    ] {
        writeln!(
            cfg,
            "\n[[governance.rules]]\nid='{id}'\nselector='{selector}'\nlimits={{turns={cap}}}"
        )
        .unwrap();
    }
    if tokens {
        writeln!(cfg, "\n[[governance.rules]]\nid='tokens'\nselector={{utc_day_agent='agent'}}\nlimits={{terminal_tokens=100}}\n[[governance.allowances]]\ntarget={{agent='agent',model='good'}}\nterminal_tokens=1").unwrap();
        // Agent/day admission needs independent coverage from before today's start.
        let conn =
            rusqlite::Connection::open(cli.store.with_extension("sqlite.usage-v1.sqlite")).unwrap();
        conn.execute("UPDATE usage_coverage_v1 SET started_ms=0", [])
            .unwrap();
    }
}

#[test]
fn governed_cli_enforces_both_owners_and_retains_completed_bytes() {
    for (request, workflow) in [(1, 2), (2, 1), (0, 2)] {
        let cli = Cli::new(true, false, "good");
        let mut raw = fs::read_to_string(&cli.config).unwrap();
        raw.push_str("\n[[workflows.nodes]]\nid='synth'\nagent='agent'\nmodel='good'\nprompt_text='{{review}}'\ninputs=['review']\n");
        fs::write(&cli.config, raw).unwrap();
        caps(&cli, request, workflow, false);
        let result = cli.command().output().unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{stderr}");
        assert!(stderr.contains("budget refused"), "{stderr}");
        assert!(!stderr.contains("InvalidStateTransition"), "{stderr}");
        assert!(!stderr.contains("LimitExceeded"), "{stderr}");
        assert_eq!(cli.prompts().len(), usize::from(request > 0));
        let path = stderr
            .lines()
            .find_map(|s| s.strip_prefix("[workflow] completed output retained at "))
            .unwrap();
        let manifest: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&fs::read(PathBuf::from(path).join("completed.json")).unwrap())
                .unwrap();
        if request > 0 {
            assert_eq!(
                fs::read(PathBuf::from(path).join(&manifest["review"])).unwrap(),
                b"answer"
            );
        } else {
            assert!(manifest.is_empty());
        }
        assert!(cli.rows().iter().all(|r| r.finalized));
        fs::remove_dir_all(path).unwrap();
    }
}

#[test]
fn governed_cli_overrun_succeeds_once_and_other_routes_refuse_before_prompt() {
    let cli = Cli::new(true, false, "good");
    // Establish coverage without a provider turn.
    let store = SqliteStore::open_shared_history(&cli.store).unwrap();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(store.usage_summary_v1(&Default::default()))
        .unwrap();
    drop(store);
    caps(&cli, 1, 1, true);
    let result = cli.command().output().unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert_eq!(
        stderr
            .matches("governed turn completed above its token allowance")
            .count(),
        1,
        "{stderr}"
    );
    assert_eq!(
        cli.rows()[0].usage.terminal.as_ref().unwrap().total_tokens,
        15
    );
    assert_eq!(cli.prompts().len(), 1);
    for route in ["serve", "mcp", "models"] {
        let result = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .args([route, "--config"])
            .arg(&cli.config)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("only local run-workflow supports caps"),
            "{route}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(cli.prompts().len(), 1);
    }
}

fn parallel_budget_fixture(cli: &Cli) -> PathBuf {
    let adapter = cli.dir.path().join("fixture-acp");
    let release = cli.dir.path().join("release-sibling");
    let script = fs::read_to_string(&adapter).unwrap()
        .replace(r#"*'"method":"initialize"'*)"#, r#"*'"method":"initialize"'*) if [ "$2" = refused ]; then while [ ! -s "$1" ]; do sleep 0.01; done; fi;"#)
        .replace(r#"printf '%s\n' "$line" >> "$1""#, r#"printf '%s\n' "$line" >> "$1"; if [ "$2" = sibling ]; then while [ ! -e "$3" ]; do sleep 0.01; done; fi"#);
    fs::write(&adapter, script).unwrap();
    text_file_busy::warm_up_script(&adapter);
    let raw = fs::read_to_string(&cli.config).unwrap().replace(
        &format!("args = [{:?}]", cli.prompts),
        &format!("args = [{:?}, 'sibling', {:?}]", cli.prompts, release),
    );
    fs::write(&cli.config, format!("{raw}\n[[agents]]\nid='other'\ncmd={adapter:?}\nargs=[{:?}, 'refused']\npre_authenticated=true\n[[workflows.nodes]]\nid='refused'\nagent='other'\nmodel='good'\nprompt_text='{{{{input}}}}'\ninputs=[]\n[[workflows.nodes]]\nid='synth'\nagent='agent'\nmodel='good'\nprompt_text='{{{{review}}}}{{{{refused}}}}'\ninputs=['review','refused']\n", cli.prompts)).unwrap();
    release
}

fn release_after_refusal(cli: &Cli, release: PathBuf) -> std::process::Output {
    use std::io::{BufRead, Read};
    let emergency_release = release.clone();
    let mut child = cli
        .command()
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut reader = std::io::BufReader::new(stderr);
        loop {
            let start = bytes.len();
            if reader.read_until(b'\n', &mut bytes).unwrap() == 0 {
                break;
            }
            if bytes[start..].starts_with(b"[workflow] node refused failed") {
                fs::write(&release, b"finish").unwrap();
            }
        }
        bytes
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            fs::write(&emergency_release, b"timeout").unwrap();
            fs::write(&cli.prompts, b"timeout").unwrap();
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "CLI budget drain did not finish: {}",
                String::from_utf8_lossy(&reader.join().unwrap())
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    std::process::Output {
        status,
        stdout,
        stderr: reader.join().unwrap(),
    }
}

#[tokio::test]
async fn governed_policy_stops_finalize_real_history_and_only_budget_evidence_waives_trigger() {
    use bridge_core::{execution_policy::*, workflow_history::*};
    for policy in ["fail_fast", "fixed_grace"] {
        for mode in ["zero", "sibling-success", "sibling-failure"] {
            let parallel = mode != "zero";
            let cli = Cli::new(true, false, "good");
            let release = parallel.then(|| parallel_budget_fixture(&cli));
            if mode == "sibling-failure" {
                let adapter = cli.dir.path().join("fixture-acp");
                let script = fs::read_to_string(&adapter).unwrap().replace(
                    r#""result":{"stopReason":"end_turn","usage":{"totalTokens":15,"inputTokens":10,"outputTokens":5}}"#,
                    r#""error":{"code":-32603,"message":"fixture sibling failed"}"#,
                );
                fs::write(&adapter, script).unwrap();
                text_file_busy::warm_up_script(&adapter);
            }
            let raw = fs::read_to_string(&cli.config).unwrap().replace(
                "id = \"flow\"",
                &format!(
                    "id = \"flow\"\nfan_out_policy='{policy}'{}",
                    if policy == "fixed_grace" {
                        "\nfixed_grace_ms=100"
                    } else {
                        ""
                    }
                ),
            );
            fs::write(&cli.config, raw).unwrap();
            caps(&cli, u64::from(parallel), 5, false);
            let result = release.map_or_else(
                || cli.command().output().unwrap(),
                |release| release_after_refusal(&cli, release),
            );
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(!result.status.success());
            assert!(stderr.contains("budget refused"), "{stderr}");
            assert!(!stderr.contains("telemetry_unavailable"), "{stderr}");
            assert_eq!(cli.prompts().len(), usize::from(parallel));
            let (_, attempt) = locator(&result);
            let attempt = AttemptId::parse(attempt).unwrap();
            let history = SqliteStore::open_history_read_only(&cli.store).unwrap();
            let record = history.attempt(&attempt).await.unwrap().unwrap();
            let terminal = record.terminal.expect("drained attempt was left active");
            assert_eq!(terminal.outcome, "failed");
            assert!(terminal.telemetry_complete);
            assert_eq!(
                terminal.node_counts.failed,
                if mode == "sibling-failure" { 2 } else { 1 }
            );
            let evidence = history
                .structured_evidence_v2(&attempt)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(evidence.node_terminals.len(), if parallel { 3 } else { 1 });
            assert!(evidence.policy_trigger_json.is_none());
            // Replay the real evidence through both history writers; a fabricated ordinary
            // failure or acceptance claim must not obtain the budget-stop exemption.
            for mutation in 0..4 {
                let memory = MemoryWorkflowHistoryStore::default();
                let sqlite = SqliteStore::open_in_memory().unwrap();
                for store in [&memory as &dyn WorkflowHistoryStore, &sqlite] {
                    store.reserve_v2(&evidence.reservation).await.unwrap();
                    for persisted in &evidence.node_terminals {
                        let mut node =
                            NodeTerminalV1::decode_canonical(persisted.terminal_json.as_bytes())
                                .unwrap();
                        if bridge_core::governance::is_budget_stop_terminal_v1(&node) {
                            match mutation {
                                0 => {}
                                1 => {
                                    node.cause = Some(NodeCauseV1::from_bridge_error(
                                        &bridge_core::error::BridgeError::StoreFailure,
                                    ))
                                }
                                2 => node.prompt_may_have_been_accepted = true,
                                _ => node.cause.as_mut().unwrap().cause_truncated = true,
                            }
                        }
                        store
                            .commit_node_terminal_v2(
                                &attempt,
                                &persisted.node,
                                &String::from_utf8(node.encode_canonical().unwrap()).unwrap(),
                                None,
                            )
                            .await
                            .unwrap();
                    }
                    assert_eq!(
                        store.terminalize(&attempt, &terminal).await.is_ok(),
                        mutation == 0
                    );
                }
            }
            let retained = stderr
                .lines()
                .find_map(|line| line.strip_prefix("[workflow] completed output retained at "))
                .unwrap();
            let outputs: std::collections::BTreeMap<String, String> = serde_json::from_slice(
                &fs::read(PathBuf::from(retained).join("completed.json")).unwrap(),
            )
            .unwrap();
            if mode == "sibling-success" {
                assert_eq!(
                    fs::read(PathBuf::from(retained).join(&outputs["review"])).unwrap(),
                    b"answer"
                );
            } else {
                assert!(outputs.is_empty());
            }
            fs::remove_dir_all(retained).unwrap();
        }
    }
}
