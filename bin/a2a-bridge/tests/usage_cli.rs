//! Reporting regressions use real stores and typed accounting writes; no SQL fixtures.
#![cfg(unix)]
use bridge_core::{accounting::*, ids::*, orch::*, task_store::TaskStore};
use bridge_store::sqlite::SqliteStore;
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
    sync::{Arc, Mutex},
};

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
}
fn text(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}
fn err(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}
fn report(path: &Path, args: &[&str]) -> Output {
    command()
        .args(["usage", "--store"])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}
fn attribution(agent: &str, model: Option<&str>, node: &str) -> UsageAttributionV1 {
    UsageAttributionV1 {
        agent: agent.into(),
        effective_model: model.map(Into::into),
        node: Some(node.into()),
        workflow: Some("flow".into()),
        ..Default::default()
    }
}
async fn record(
    store: &SqliteStore,
    owner: &AttemptIdentity,
    mut a: UsageAttributionV1,
    day: i64,
    state: &str,
    usage: UsageSnapshot,
) {
    a.execution = Some(owner.execution_id.as_str().into());
    a.run_attempt = Some(owner.attempt_id.as_str().into());
    let row = UsageAttemptV1 {
        id: AttemptId::mint().unwrap().as_str().into(),
        attribution: a,
        started_ms: day * 86_400_000,
        utc_day: day,
        dispatched: false,
        finalized: false,
        usage: Default::default(),
        cost_invalid: false,
    };
    store.start_usage_attempt_v1(&row).await.unwrap();
    store
        .update_usage_attempt_v1(
            &row.id,
            state != "preflight",
            Some(usage),
            state != "pending",
        )
        .await
        .unwrap();
}
fn measured(tokens: u64, amount: f64, currency: &str) -> UsageSnapshot {
    UsageSnapshot {
        terminal: Some(TerminalUsage {
            total_tokens: tokens,
            input_tokens: tokens,
            output_tokens: 0,
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
        }),
        cost: Some(UsageCost {
            amount,
            currency: currency.into(),
        }),
        ..Default::default()
    }
}
fn parsed(out: &Output) -> Value {
    assert!(out.status.success(), "{}", err(out));
    serde_json::from_slice(&out.stdout).unwrap()
}
fn usage_lines(out: &Output) -> Vec<String> {
    err(out)
        .lines()
        .filter(|s| s.starts_with("usage "))
        .map(Into::into)
        .collect()
}

#[tokio::test]
async fn filters_grouping_options_and_typed_cost_quality() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.sqlite");
    let store = SqliteStore::open_shared_history(&path).unwrap();
    let owner = AttemptIdentity::initial().unwrap();
    // Unix day 20745 = 2026-10-19. Include sentinel-looking legitimate identifiers.
    for (agent, model, day, state, snapshot) in [
        (
            "unknown",
            Some("invalid"),
            20745,
            "done",
            measured(0, 0.0, "USD"),
        ),
        (
            "unknown",
            Some("invalid"),
            20745,
            "pending",
            UsageSnapshot::default(),
        ),
        (
            "unknown",
            Some("invalid"),
            20745,
            "preflight",
            measured(3, -5.0, "USD"),
        ),
        (
            "unknown",
            Some("invalid"),
            20745,
            "done",
            measured(4, 1.25, "EUR"),
        ),
        (
            "unknown",
            Some("invalid"),
            20745,
            "done",
            UsageSnapshot::default(),
        ),
        ("other", None, 20746, "done", measured(0, 0.0, "USD")),
        ("b", Some("m"), 20747, "done", measured(2, 0.0, "USD")),
    ] {
        let mut a = attribution(agent, model, "review");
        if agent == "other" {
            a.workflow = Some("other-flow".into());
            a.billing = Some(BillingIdentityV1 {
                billing_mode: "subscription".into(),
                account_group: "account".into(),
            });
        }
        record(&store, &owner, a, day, state, snapshot).await;
    }
    let filters = [
        "--json",
        "--agent",
        "unknown",
        "--model",
        "invalid",
        "--workflow",
        "flow",
        "--day",
        "2026-10-19",
    ];
    let out = report(&path, &filters);
    let v = parsed(&out);
    let b = &v["buckets"][0];
    assert_eq!(v["buckets"].as_array().unwrap().len(), 1);
    assert_eq!(b["keys"]["agent"], "unknown");
    assert_eq!(b["keys"]["effective_model"], "invalid");
    assert_eq!(
        (
            b["attempted"].clone(),
            b["dispatched"].clone(),
            b["pending"].clone()
        ),
        (json!(5), json!(4), json!(1))
    );
    assert_eq!(
        b["total_tokens"],
        json!({"reported_sum":7,"missing_count":1,"partial":true})
    );
    assert_eq!(b["cost_by_currency"], json!({"USD":0.0,"EUR":1.25}));
    assert_eq!(b["cost_invalid"], 1);
    assert_eq!(b["cost_missing"], 1);
    assert!(b["keys"]["billing"].is_null());
    let human = text(&report(&path, &filters[1..filters.len()])); // remove --json
    assert!(
        human.contains("billing=unmapped")
            && human.contains("pending=1")
            && human.contains("missing_count:1")
    );
    let all = parsed(&report(&path, &["--json", "--group-by", "agent"]));
    assert_eq!(all["buckets"].as_array().unwrap().len(), 3);
    assert!(all["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["keys"]["effective_model"].is_null()));
    let zero = parsed(&report(&path, &["--json", "--agent", "other"]));
    let b = &zero["buckets"][0];
    assert_eq!(
        b["total_tokens"],
        json!({"reported_sum":0,"missing_count":0,"partial":false})
    );
    for key in ["pending", "unmapped", "cost_missing", "cost_invalid"] {
        assert_eq!(b[key], 0);
    }
    let human = text(&report(&path, &["--agent", "other"]));
    assert!(
        human.contains("billing={\"billing_mode\":\"subscription\",\"account_group\":\"account\"}")
    );
    assert!(!human.contains("billing=unmapped"));
    for (flag, value) in [
        ("--execution", "absent"),
        ("--attempt", "absent"),
        ("--node", "absent"),
        ("--workflow", "absent"),
        ("--model", "absent"),
        ("--agent", "absent"),
        ("--day", "2026-10-18"),
    ] {
        assert!(parsed(&report(&path, &["--json", flag, value]))["buckets"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    let exact = parsed(&report(
        &path,
        &[
            "--json",
            "--execution",
            owner.execution_id.as_str(),
            "--attempt",
            owner.attempt_id.as_str(),
            "--node",
            "review",
            "--from-day",
            "2026-10-19",
            "--through-day",
            "2026-10-19",
            "--group-by",
            "model",
        ],
    ));
    assert_eq!(exact["buckets"][0]["attempted"], 5);
    assert!(!report(&path, &["--day", "2026-02-30"]).status.success());
    assert!(!report(
        &path,
        &["--from-day", "2026-10-20", "--through-day", "2026-10-19"]
    )
    .status
    .success());
}

#[tokio::test]
async fn empty_coverage_and_noncreating_admission() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.sqlite");
    let store = SqliteStore::open_shared_history(&path).unwrap();
    store.usage_summary_v1(&Default::default()).await.unwrap();
    for (args, expected) in [
        (vec!["--day", "9999-01-01"], "covered-empty"),
        (vec!["--day", "1970-01-01"], "before-coverage"),
        (vec!["--node", "absent"], "historical-incomplete"),
    ] {
        let mut json_args = args.clone();
        json_args.push("--json");
        let value = parsed(&report(&path, &json_args));
        assert_eq!(value["empty_state"], expected);
        assert_eq!(value["historical_incomplete"], true);
        assert!(text(&report(&path, &args)).contains(expected));
    }
    let unrelated = dir.path().join("unrelated.sqlite");
    fs::write(&unrelated, "unrelated").unwrap();
    let legacy = dir.path().join("legacy.sqlite");
    drop(SqliteStore::open(&legacy).unwrap());
    for bad in [
        dir.path().join("missing.sqlite"),
        unrelated,
        legacy,
        dir.path().to_owned(),
    ] {
        let out = report(&bad, &["--json"]);
        assert!(!out.status.success());
        assert!(text(&out).is_empty());
        assert!(err(&out).contains("usage unavailable:"));
        assert!(!Path::new(&format!("{}.usage-v1.sqlite", bad.display())).exists());
    }
    // An incompatible sidecar must be refused rather than initialized/repaired.
    drop(store);
    let sidecar = path.with_extension("sqlite.usage-v1.sqlite");
    fs::write(&sidecar, "incompatible").unwrap();
    assert!(!report(&path, &[]).status.success());
    assert_eq!(fs::read_to_string(sidecar).unwrap(), "incompatible");
    let no_sidecar = dir.path().join("old.sqlite");
    drop(SqliteStore::open_shared_history(&no_sidecar).unwrap());
    for args in [
        vec!["--json"],
        vec!["--json", "--day", "2026-10-19"],
        vec!["--json", "--day", "1969-01-01"],
        vec!["--json", "--from-day", "2026-10-19"],
    ] {
        assert_eq!(
            parsed(&report(&no_sidecar, &args))["empty_state"],
            "historical-incomplete"
        );
    }
    assert!(!no_sidecar.with_extension("sqlite.usage-v1.sqlite").exists());
}

#[derive(Clone)]
struct Server {
    store: Arc<SqliteStore>,
    mode: &'static str,
    requests: Arc<Mutex<Vec<Value>>>,
}
async fn respond(
    axum::extract::State(s): axum::extract::State<Server>,
    axum::Json(request): axum::Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    s.requests.lock().unwrap().push(request.clone());
    let md = &request["params"]["message"]["metadata"];
    let owner = AttemptIdentity {
        execution_id: ExecutionId::parse(md["a2a-bridge.execution_id"].as_str().unwrap()).unwrap(),
        attempt_id: AttemptId::parse(md["a2a-bridge.attempt_id"].as_str().unwrap()).unwrap(),
        ordinal: 0,
        parent_attempt_id: None,
    };
    if s.mode != "unmatched" {
        record(
            &s.store,
            &owner,
            attribution("actual", Some("model"), "review"),
            20745,
            "done",
            measured(7, 0.5, "USD"),
        )
        .await;
    }
    if request["method"] == a2a::methods::SEND_MESSAGE {
        return axum::Json(if s.mode == "json-error" {
            json!({"error":{"message":"original failure"}})
        } else {
            json!({"result":{"artifact":{"text":"answer\n"}}})
        })
        .into_response();
    }
    if s.mode == "json-error" {
        return axum::Json(json!({"error":{"message":"original failure"}})).into_response();
    }
    if s.mode == "malformed" {
        return ([("content-type", "text/event-stream")], "data: invalid\n\n").into_response();
    }
    if s.mode == "stream-error" {
        let stream = futures::stream::iter([
            Ok::<_, std::io::Error>("data: ".to_owned()),
            Err(std::io::Error::other("original stream failure")),
        ]);
        return axum::response::Response::builder()
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from_stream(stream))
            .unwrap();
    }
    let artifact = json!({"artifactUpdate":{"taskId":"task","contextId":"ctx",
        "artifact":{"artifactId":"artifact","parts":[{"text":"answer\n"}]}}});
    let terminal = json!({"statusUpdate":{"taskId":"task","contextId":"ctx",
        "status":{"state":if s.mode == "failed" { "TASK_STATE_FAILED" } else { "TASK_STATE_COMPLETED" }}}});
    let body = format!(
        "data: {}\n\ndata: {}\n\n",
        serde_json::to_string(&artifact).unwrap(),
        serde_json::to_string(&terminal).unwrap()
    );
    ([("content-type", "text/event-stream")], body).into_response()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clients_report_once_on_all_exits_and_keep_wire_and_output_bytes() {
    for served in [false, true] {
        for mode in [
            "ok",
            "json-error",
            "malformed",
            "stream-error",
            "failed",
            "unmatched",
        ] {
            if !served && ["malformed", "stream-error", "failed"].contains(&mode) {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let actual = dir.path().join("actual");
            let links = dir.path().join("links");
            fs::create_dir(&actual).unwrap();
            fs::create_dir(&links).unwrap();
            let path = actual.join("store.sqlite");
            // A valid unrelated store beside the symlink must never supply client usage.
            drop(SqliteStore::open_shared_history(&links.join("store.sqlite")).unwrap());
            let store = Arc::new(SqliteStore::open_shared_history(&path).unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let router = axum::Router::new()
                .route("/", axum::routing::post(respond))
                .with_state(Server {
                    store,
                    mode,
                    requests: requests.clone(),
                });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            let input = dir.path().join("input.md");
            fs::write(&input, "---\ntask-type: freeform\n---\nbrief").unwrap();
            let config = actual.join("config.toml");
            fs::write(
                &config,
                "default=\"unused\"\n[server]\naddr=\"127.0.0.1:0\"\n[[agents]]\nid=\"unused\"\ncmd=\"unused\"\n[store]\npath=\"store.sqlite\"\n",
            )
            .unwrap();
            let link = links.join("config.toml");
            std::os::unix::fs::symlink(&config, &link).unwrap();
            let mut previous_wire = None;
            for selection in ["selected", "config", "unselected", "missing", "out"] {
                let mut cmd = command();
                if served {
                    cmd.args(["run-workflow", "flow", "--serve", "--context", "ctx"]);
                } else {
                    cmd.args(["submit", "flow", "--context", "ctx"]);
                }
                cmd.args(["--no-heartbeat", "--url", &url, "--input"])
                    .arg(&input);
                match selection {
                    "selected" | "out" => {
                        cmd.arg("--usage-store").arg(&path);
                    }
                    "config" => {
                        cmd.arg("--usage-config").arg(&link);
                    }
                    "missing" => {
                        cmd.arg("--usage-store")
                            .arg(dir.path().join("missing.sqlite"));
                    }
                    _ => {}
                }
                let out_path = dir.path().join("answer");
                if served && selection == "out" {
                    cmd.arg("--out").arg(&out_path);
                }
                let out = cmd.output().unwrap();
                // Reporting failures must preserve a successful submit and its stdout payload.
                if !served
                    && mode == "ok"
                    && ["selected", "unselected"].contains(&selection)
                    && Path::new("/dev/full").exists()
                {
                    let full = cmd
                        .stderr(
                            fs::OpenOptions::new()
                                .write(true)
                                .open("/dev/full")
                                .unwrap(),
                        )
                        .output()
                        .unwrap();
                    assert!(full.status.success());
                    assert_eq!(
                        text(&full).lines().skip(2).collect::<Vec<_>>(),
                        text(&out).lines().skip(2).collect::<Vec<_>>()
                    );
                }
                let lines = usage_lines(&out);
                assert_eq!(lines.len(), 1, "{}", err(&out));
                let matched =
                    ["selected", "config", "out"].contains(&selection) && mode != "unmatched";
                assert_eq!(
                    lines[0].starts_with("usage execution="),
                    matched,
                    "{}",
                    lines[0]
                );
                if matched {
                    assert!(
                        lines[0].contains("attempts=1") && lines[0].contains("agent=\"actual\"")
                    );
                }
                let success = mode == "ok" || mode == "unmatched";
                assert_eq!(out.status.success(), success, "{}", err(&out));
                if mode == "json-error" {
                    assert!(err(&out).contains("original failure"));
                }
                if mode == "malformed" {
                    assert!(err(&out).contains("bad SSE data frame"));
                }
                if mode == "failed" {
                    assert!(err(&out).contains("workflow did not complete successfully"));
                }
                let payload = text(&out).lines().skip(2).collect::<Vec<_>>().join("\n");
                if success || mode == "failed" {
                    if served && selection == "out" {
                        assert_eq!(fs::read(&out_path).unwrap(), b"answer\n");
                        assert!(payload.is_empty());
                    } else {
                        assert_eq!(payload, if served { "answer" } else { "answer\n" });
                    }
                }
                let mut wire = requests.lock().unwrap().last().unwrap().clone();
                let m = &mut wire["params"]["message"];
                m["messageId"] = json!("id");
                m["taskId"] = json!("task");
                m["metadata"]["a2a-bridge.execution_id"] = json!("exec");
                m["metadata"]["a2a-bridge.attempt_id"] = json!("attempt");
                if let Some(previous) = &previous_wire {
                    assert_eq!(&wire, previous);
                }
                previous_wire = Some(wire);
                assert!(!dir.path().join("missing.sqlite").exists());
            }
            server.abort();
        }
    }
}

#[tokio::test]
async fn local_nodes_count_retries_and_use_symlink_config_directory() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let actual = dir.path().join("actual");
    fs::create_dir(&actual).unwrap();
    let adapter = dir.path().join("adapter");
    // Fail once during session preparation, then succeed: this is a permitted cold retry.
    fs::write(&adapter, r#"#!/usr/bin/env python3
import json,sys,os
for line in sys.stdin:
    r=json.loads(line); m=r['method']; result={}
    if m=='initialize': result={'protocolVersion':1,'agentCapabilities':{},'authMethods':[]}
    elif m=='session/new':
        if not os.path.exists(sys.argv[1]):
            open(sys.argv[1],'w').close()
            print(json.dumps({'jsonrpc':'2.0','id':r['id'],'error':{'code':-32603,'message':'synthetic retry'}}),flush=True); continue
        result={'sessionId':'fixture','models':{'availableModels':[{'modelId':'actual','name':'Actual'}],'currentModelId':'actual'}}
    elif m=='session/prompt':
        print(json.dumps({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'answer'}}}}),flush=True)
        result={'stopReason':'end_turn','usage':{'totalTokens':5,'inputTokens':3,'outputTokens':2}}
    if 'id' in r: print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}),flush=True)
"#).unwrap();
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
    let config = actual.join("config.toml");
    fs::write(
        &config,
        format!(
            r#"default="agent"
[store]
path="store.sqlite"
[registry]
allowed_cmds=[{adapter:?}]
[[agents]]
id="agent"
cmd={adapter:?}
args=[{marker:?}]
pre_authenticated=true
[server]
addr="127.0.0.1:0"
[[workflows]]
id="flow"
[[workflows.nodes]]
id="draft"
agent="agent"
model="actual"
prompt_text="{{{{input}}}}"
inputs=[]
retry={{max_attempts=2,backoff_ms=0}}
[[workflows.nodes]]
id="review"
agent="agent"
model="actual"
prompt_text="{{{{draft}}}}"
inputs=["draft"]
"#,
            marker = dir.path().join("once")
        ),
    )
    .unwrap();
    let link = dir.path().join("config.toml");
    symlink(&config, &link).unwrap();
    let input = dir.path().join("input.md");
    fs::write(&input, "---\ntask-type: freeform\n---\nbrief").unwrap();
    let output_path = dir.path().join("answer");
    let out = command()
        .args(["run-workflow", "flow", "--no-heartbeat", "--config"])
        .arg(&link)
        .arg("--input")
        .arg(input)
        .arg("--out")
        .arg(&output_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", err(&out));
    assert_eq!(fs::read(output_path).unwrap(), b"answer");
    assert_eq!(text(&out).lines().count(), 2); // Only the pre-existing identity locators.
    let lines = usage_lines(&out);
    assert_eq!(lines.len(), 2, "{}", err(&out));
    assert!(
        lines[0].contains("node=\"draft\"") && lines[0].contains("attempts=2"),
        "{}",
        lines[0]
    );
    assert!(lines[1].contains("node=\"review\"") && lines[1].contains("attempts=1"));
    assert!(lines.iter().all(|l| l.contains("agent=\"agent\"")));
    assert!(lines.iter().any(|l| l.contains("model=\"actual\"")));
    let path = dir.path().join("store.sqlite");
    assert!(path.exists());
    assert!(!actual.join("store.sqlite").exists());
    let report = command()
        .args(["usage", "--config"])
        .arg(link)
        .arg("--json")
        .output()
        .unwrap();
    let value = parsed(&report);
    assert_eq!(
        value["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["attempted"].as_u64().unwrap())
            .sum::<u64>(),
        3
    );
    // A nonexistent node never gets an invented attempt.
    assert!(parsed(&self::report(
        &path,
        &["--json", "--node", "not-dispatched"]
    ))["buckets"]
        .as_array()
        .unwrap()
        .is_empty());
}
