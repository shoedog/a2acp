//! Executable fixture controls; no provider credentials or live models.
#![cfg(unix)]
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Direct {
    dir: tempfile::TempDir,
    child: Child,
    output: Option<BufReader<std::process::ChildStdout>>,
    url: String,
    peer: std::net::TcpListener,
}
impl Direct {
    fn new(mode: &str, caps: &str) -> Self {
        Self::seeded(mode, caps, false)
    }
    fn seeded(mode: &str, caps: &str, recovery: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let adapter = dir.path().join("fixture-acp");
        fs::write(&adapter, include_str!("accounting/fixtures/direct-acp")).unwrap();
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let peer = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        peer.set_nonblocking(true).unwrap();
        let peer_addr = peer.local_addr().unwrap();
        let config = format!("default='agent'\nallowed_cwd_root={root:?}\n[store]\npath='tasks.sqlite'\n[batch]\nmax_concurrent=1\n[delegation]\npeer_url='http://{peer_addr}'\nauth='fixture'\n[[agents]]\nid='agent'\ncmd={adapter:?}\nargs=['prompts']\nmodel='good'\npre_authenticated=true\n[[billing_identities]]\nagent='agent'\nmodel='good'\naccount_group='fixture'\nbilling_mode='metered'\n[[workflows]]\nid='flow'\n[[workflows.nodes]]\nid='review'\nagent='agent'\nprompt_text='{{{{input}}}}'\ninputs=[]\n[server]\naddr='{addr}'\n", root=dir.path());
        fs::write(dir.path().join("base"), &config).unwrap();
        fs::write(dir.path().join("config.toml"), format!("{config}{caps}")).unwrap();
        if recovery {
            use bridge_core::{ids::TaskId, task_store::*};
            let store =
                bridge_store::sqlite::SqliteStore::open(&dir.path().join("tasks.sqlite")).unwrap();
            futures::executor::block_on(store.create(&TaskRecord {
                id: TaskId::parse("outstanding").unwrap(),
                workflow: "flow".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: 1,
                updated_ms: 1,
                last_artifact_ms: None,
                input: "preserve".into(),
                workflow_spec_json: None,
                resume_attempts: 0,
                session_cwd: None,
                batch_id: None,
                item_id: None,
                artifacts_purged_at: None,
            }))
            .unwrap();
        }
        let stderr = fs::File::create(dir.path().join("stderr")).unwrap();
        let mut child = Command::new(
            std::env::var_os("A2A_GMIN5B_TEST_BINARY")
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_a2a-bridge").into()),
        )
        .args([mode, "--config", "config.toml"])
        .current_dir(dir.path())
        .env_remove("A2A_BRIDGE_MCP_CALL_DEPTH")
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .unwrap();
        let output = child.stdout.take().map(BufReader::new);
        let mut f = Self {
            dir,
            child,
            output,
            url: format!("http://{addr}"),
            peer,
        };
        if mode == "serve" {
            for _ in 0..400 {
                assert!(
                    f.child.try_wait().unwrap().is_none(),
                    "startup failed: {}",
                    f.stderr()
                );
                if std::net::TcpStream::connect(addr).is_ok() {
                    return f;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            panic!("serve startup timed out: {}", f.stderr());
        }
        writeln!(
            f.child.stdin.as_mut().unwrap(),
            "{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}"
        )
        .unwrap();
        f
    }
    fn stderr(&self) -> String {
        fs::read_to_string(self.dir.path().join("stderr")).unwrap()
    }
    fn edit(&self, caps: &str) {
        let base = fs::read_to_string(self.dir.path().join("base")).unwrap();
        fs::write(self.dir.path().join("config.toml"), format!("{base}{caps}")).unwrap();
        std::thread::sleep(Duration::from_millis(650));
    }
    fn mcp(&mut self, tool: &str, n: u32) -> String {
        let mut args = json!({"input":"go", "text":"note", "context":"warm", "model":"good", "execution_id":format!("exec-{n:032}"), "attempt_id":format!("attempt-{n:032}")});
        if tool == "run_workflow" {
            args["workflow"] = json!("flow");
            args["input"] = json!("---\ntask-type: freeform\n---\ngo");
            args.as_object_mut().unwrap().remove("context");
            args.as_object_mut().unwrap().remove("model");
        }
        let call = json!({"jsonrpc":"2.0", "id":n, "method":"tools/call", "params":{"name":tool,"arguments":args}});
        writeln!(self.child.stdin.as_mut().unwrap(), "{call}").unwrap();
        let mut line = String::new();
        self.output.as_mut().unwrap().read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "MCP exited: {}", self.stderr());
        line
    }
    async fn a2a(&self, method: &str, n: u32, model: &str) -> String {
        let mut body = json!({"jsonrpc":"2.0", "id":n, "method":method,"params":{
            "workflow":"flow", "items":[{"input":"go"}], "contextId":"warm", "text":"note", "message":{"text":"go", "contextId":"warm", "taskId":format!("exec-{n:032}"),
            "metadata":{"a2a-bridge.model":model,"a2a-bridge.execution_id":format!("exec-{n:032}"), "a2a-bridge.attempt_id":format!("attempt-{n:032}")}}}});
        if method == "RunBatch" {
            body["params"]["items"][0]["input"] = json!("---\ntask-type: freeform\n---\ngo");
        }
        if ["flow", "fan-out"].contains(&model) {
            body["params"].as_object_mut().unwrap().remove("contextId");
            body["params"]["message"]
                .as_object_mut()
                .unwrap()
                .remove("contextId");
            body["params"]["message"]["text"] = json!("---\ntask-type: freeform\n---\ngo");
            body["params"]["message"]["metadata"]["a2a-bridge.skill"] = json!(model);
        }
        let response = reqwest::Client::new()
            .post(&self.url)
            .header("A2A-Version", "1.0")
            .json(&body)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        response
    }
    fn prompts(&self) -> usize {
        fs::read_to_string(self.dir.path().join("prompts"))
            .unwrap_or_default()
            .lines()
            .count()
    }
    fn records(&self) -> Vec<String> {
        let path = self.dir.path().join("tasks.sqlite.usage-v1.sqlite");
        let conn = rusqlite::Connection::open(path).unwrap();
        let mut query = conn
            .prepare("SELECT record_json FROM usage_attempts_v1 ORDER BY rowid")
            .unwrap();
        query
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
}
impl Drop for Direct {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
const P: &str = "\n[[governance.rules]]\nid='request'\nselector='each_request'\nlimits={turns=1}\n[[governance.rules]]\nid='daily'\nselector='each_workflow_run'\nlimits={turns=2}\n";

#[tokio::test]
async fn activated_entrypoints_and_policy_reload_controls() {
    let f = Direct::new("serve", P);
    assert!(f.a2a("SendMessage", 1, "good").await.contains("answer"));
    let base_path = f.dir.path().join("base");
    let original = fs::read_to_string(&base_path).unwrap();
    fs::write(
        &base_path,
        original.replace("account_group='fixture'", "account_group='rejected'"),
    )
    .unwrap();
    let q = P.replace("turns=2", "turns=0");
    f.edit(&q);
    f.edit("");
    unsafe {
        libc::kill(f.child.id() as i32, libc::SIGHUP);
    }
    assert_eq!(f.stderr().matches("governance reload refused").count(), 2);
    assert!(f
        .a2a("SendStreamingMessage", 2, "good")
        .await
        .contains("answer"));
    assert!(f
        .records()
        .iter()
        .all(|r| r.contains("\"account_group\":\"fixture\"")));
    fs::write(
        &base_path,
        original.replace("account_group='fixture'", "account_group='accepted'"),
    )
    .unwrap();
    let rules: Vec<_> = P.split("[[governance.rules]]").skip(1).collect();
    f.edit(&format!(
        "[[governance.rules]]{}[[governance.rules]]{}",
        rules[1], rules[0]
    ));
    assert!(f.a2a("SendMessage", 3, "good").await.contains("answer"));
    assert!(f
        .records()
        .last()
        .unwrap()
        .contains("\"account_group\":\"accepted\""));
    assert_eq!(f.stderr().matches("governance reload refused").count(), 2);
    assert_eq!(f.prompts(), 3);
    assert_eq!(
        f.stderr().matches("recovery disabled under caps").count(),
        1
    );
    let mut m = Direct::new("mcp", P);
    assert!(m.mcp("run", 1).contains("answer"));
    assert!(m.mcp("continue", 2).contains("answer"));
    assert_eq!(m.prompts(), 2);
    let mut zero = Direct::new("mcp", &P.replace("turns=1", "turns=0"));
    assert!(zero.mcp("run", 1).contains("budget refused"));
    assert_eq!(zero.prompts(), 0);
    let uncapped = Direct::new("serve", "");
    uncapped.edit(P);
    assert_eq!(
        uncapped
            .stderr()
            .matches("governance reload refused")
            .count(),
        1
    );
    assert!(uncapped
        .a2a("SendMessage", 1, "good")
        .await
        .contains("answer"));
}

#[tokio::test]
async fn unsupported_entrypoints() {
    let f = Direct::new("serve", P);
    for method in ["SessionCompact", "SessionInject", "RunBatch"] {
        assert!(f.a2a(method, 1, "good").await.contains("budget refused"));
    }
    for skill in ["flow", "fan-out"] {
        let response = f.a2a("SendMessage", 1, skill).await;
        assert!(response.contains("budget refused"), "{response}");
    }
    let mut m = Direct::new("mcp", P);
    for tool in ["inject", "run_workflow", "batch"] {
        let result = m.mcp(tool, 1);
        assert!(!result.contains("answer"), "{result}");
    }
    assert_eq!(m.prompts(), 0);
    assert!(matches!(f.peer.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    for args in [
        vec!["models"],
        vec![
            "smoke",
            "--agent",
            "agent",
            "--model",
            "good",
            "--acknowledge-billable",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .args(args)
            .args(["--config", "config.toml"])
            .current_dir(f.dir.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .replace("smoke.config_load", "unavailable during governance staging")
        .contains("unavailable during governance staging"));
    }
    assert_eq!(f.prompts(), 0);
    assert!(f.a2a("SendMessage", 1, "good").await.contains("answer"));
}

#[tokio::test]
async fn governed_startup_recovery() {
    let f = Direct::seeded("serve", P, true);
    assert_eq!(
        f.stderr().matches("recovery disabled under caps").count(),
        1
    );
    let conn = rusqlite::Connection::open(f.dir.path().join("tasks.sqlite")).unwrap();
    let row: (String, i64, i64, String) = conn
        .query_row(
            "SELECT status,updated_ms,resume_attempts,input FROM tasks WHERE id='outstanding'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row, ("working".into(), 1, 0, "preserve".into()));
    assert_eq!(f.prompts(), 0);
}

#[tokio::test]
async fn policy_reload_pending() {
    let f = Direct::new("serve", P);
    fs::write(f.dir.path().join("prompts.block"), "").unwrap();
    let pending = f.a2a("SendMessage", 1, "good");
    tokio::pin!(pending);
    tokio::select! { _ = &mut pending => panic!("fixture did not block"), _ = async { tokio::time::timeout(Duration::from_secs(5), async { while f.prompts() == 0 { tokio::time::sleep(Duration::from_millis(25)).await; } }).await.unwrap(); } => () }
    assert_eq!(f.prompts(), 1);
    f.edit(&P.replace("turns=1", "turns=0"));
    f.edit("");
    unsafe {
        libc::kill(f.child.id() as i32, libc::SIGHUP);
    }
    assert_eq!(f.stderr().matches("governance reload refused").count(), 2);
    fs::remove_file(f.dir.path().join("prompts.block")).unwrap();
    assert!(pending.await.contains("answer"));
    assert!(f.a2a("SendMessage", 2, "good").await.contains("answer"));
}

#[tokio::test]
async fn shared_store_mismatch() {
    let f = Direct::new("serve", P);
    fs::write(f.dir.path().join("prompts.block"), "").unwrap();
    {
        let pending = f.a2a("SendMessage", 1, "good");
        tokio::pin!(pending);
        tokio::select! { _ = &mut pending => panic!("fixture did not block"), _ = async { tokio::time::timeout(Duration::from_secs(5), async { while f.prompts() == 0 { tokio::time::sleep(Duration::from_millis(25)).await; } }).await.unwrap(); } => () }
        assert_eq!(f.prompts(), 1);
        unsafe {
            libc::kill(f.child.id() as i32, libc::SIGKILL);
        }
    }
    std::thread::sleep(Duration::from_millis(100));
    let path = f.dir.path().join("tasks.sqlite");
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["shared_store_probe", "--nocapture"])
        .env("A2A_GMIN5B_SHARED_STORE", &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(f.prompts(), 1);
}

#[tokio::test]
async fn shared_store_probe() {
    use bridge_core::{accounting::UsageAttemptV1, governance::*, ids::*, task_store::TaskStore};
    let Some(path) = std::env::var_os("A2A_GMIN5B_SHARED_STORE") else {
        return;
    };
    let store = bridge_store::sqlite::SqliteStore::open(std::path::Path::new(&path)).unwrap();
    let conn =
        rusqlite::Connection::open(format!("{}.usage-v1.sqlite", path.to_str().unwrap())).unwrap();
    let raw: String = conn
        .query_row("SELECT record_json FROM usage_attempts_v1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let row: UsageAttemptV1 = serde_json::from_str(&raw).unwrap();
    assert!(row.dispatched && !row.finalized);
    let context = BudgetContextV1 {
        request: BudgetRequestIdV1::Execution(
            ExecutionId::parse(row.attribution.execution.unwrap()).unwrap(),
        ),
        workflow_run: None,
    };
    let candidate = BudgetCandidateV1 {
        agent: AgentId::parse(row.attribution.agent).unwrap(),
        model: row.attribution.effective_model,
    };
    let q = BudgetPolicyV1::new(
        vec![BudgetRuleV1 {
            id: "changed".into(),
            selector: BudgetSelectorV1::EachRequest,
            limits: BudgetLimitsV1 {
                turns: Some(2),
                terminal_tokens: None,
            },
        }],
        vec![],
    )
    .unwrap();
    let before: String = conn
        .query_row("SELECT digest_json FROM governance_policy_v1", [], |r| {
            r.get(0)
        })
        .unwrap();
    for policy in [Some(&q), None] {
        assert!(matches!(
            store
                .admit_usage_attempt_v1(
                    policy,
                    &AttemptId::parse(&row.id).unwrap(),
                    &context,
                    &candidate
                )
                .await,
            BudgetAdmissionOutcomeV1::Refused(BudgetRefusalV1 {
                reason: BudgetRefusalReasonV1::PolicyMismatch,
                ..
            })
        ));
    }
    let after: String = conn
        .query_row("SELECT digest_json FROM governance_policy_v1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(before, after);
}

// Compare strings, including raw accounting JSON, never semantic payload equality.
async fn direct_projection() -> Value {
    let f = Direct::new("serve", "");
    let mut payloads = Vec::new();
    for (method, n, model) in [
        ("SendMessage", 1, "good"),
        ("SendStreamingMessage", 2, "good"),
        ("SessionCompact", 6, "good"),
        ("SessionInject", 5, "good"),
        ("SendMessage", 3, "missing"),
        ("SendStreamingMessage", 4, "missing"),
    ] {
        tokio::time::sleep(Duration::from_millis(100)).await;
        payloads.push(f.a2a(method, n, model).await);
    }
    let mut m = Direct::new("mcp", "");
    for (tool, n) in [("run", 1), ("continue", 2), ("inject", 3)] {
        payloads.push(m.mcp(tool, n));
    }
    let records = [f.records(), m.records()].concat();
    let mut replacements = Vec::new();
    // Validate dynamic fields before canonicalizing their exact raw encodings.
    fn dynamic(v: &Value, replacements: &mut Vec<(String, String)>) {
        match v {
            Value::Object(fields) => {
                for (key, value) in fields {
                    if ["started_ms", "utc_day", "at_ms"].contains(&key.as_str()) {
                        assert!(value.as_i64().unwrap() >= 0);
                        replacements.push((format!("\"{key}\":{value}"), format!("\"{key}\":0")));
                    } else {
                        dynamic(value, replacements);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    dynamic(value, replacements);
                }
            }
            _ => (),
        }
    }
    for (index, record) in records.iter().enumerate() {
        let row: Value = serde_json::from_str(record).unwrap();
        let id = row["id"].as_str().unwrap();
        bridge_core::ids::AttemptId::parse(id).unwrap();
        assert_eq!(row["attribution"]["turn"], id);
        replacements.push((id.into(), format!("attempt-{:032}", index + 100)));
        if row["attribution"]["attempt_kind"] == "compaction" {
            for key in ["execution", "run_attempt"] {
                let id = row["attribution"][key].as_str().unwrap();
                if key == "execution" {
                    bridge_core::ids::ExecutionId::parse(id).unwrap();
                } else {
                    bridge_core::ids::AttemptId::parse(id).unwrap();
                }
                replacements.push((id.into(), format!("$COMPACTION_{key}")));
            }
        }
    }
    for payload in &payloads {
        for line in payload.lines().filter_map(|l| l.strip_prefix("data: ")) {
            let frame: Value = serde_json::from_str(line).unwrap();
            for pointer in [
                "/statusUpdate/status/message/messageId",
                "/artifactUpdate/artifact/artifactId",
            ] {
                if let Some(id) = frame.pointer(pointer).and_then(Value::as_str) {
                    assert_eq!(id.len(), 36);
                    assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
                    replacements.push((id.into(), format!("$FRAME{pointer}")));
                }
            }
        }
    }
    for record in &records {
        dynamic(
            &serde_json::from_str::<Value>(record).unwrap(),
            &mut replacements,
        );
    }
    let canonicalize = |s: &str| {
        let mut text = s.to_owned();
        for root in [&f.dir, &m.dir] {
            text = text
                .replace(
                    fs::canonicalize(root.path()).unwrap().to_str().unwrap(),
                    "$FIXTURE",
                )
                .replace(root.path().to_str().unwrap(), "$FIXTURE");
        }
        // This closed-set metadata HashMap has randomized field order on the base.
        // Sort raw field spans only; preserve every key, value and formatting byte.
        if let Some(start) = text.find("\"metadata\":{") {
            let start = start + "\"metadata\":{".len();
            let end = text[start..].find('}').unwrap() + start;
            let mut spans: Vec<_> = text[start..end].split(',').collect();
            assert_eq!(spans.len(), 5);
            spans.sort_unstable();
            let ordered = spans.join(",");
            text.replace_range(start..end, &ordered);
        }
        for (from, to) in &replacements {
            text = text.replace(from, to);
        }
        text
    };
    for fixture in [&f, &m] {
        let conn =
            rusqlite::Connection::open(fixture.dir.path().join("tasks.sqlite.usage-v1.sqlite"))
                .unwrap();
        let count: u32 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE 'governance_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    json!({"payloads":payloads.iter().map(|p|canonicalize(p)).collect::<Vec<_>>(),"stderr":[canonicalize(&f.stderr()),canonicalize(&m.stderr())],"records":records.iter().map(|r|canonicalize(r)).collect::<Vec<_>>()})
}
#[tokio::test]
async fn uncapped_direct_baselines_and_baseline_discriminator() {
    let actual = direct_projection().await;
    if std::env::var_os("A2A_GMIN5B_TEST_BINARY").is_some() {
        eprintln!("direct_baseline_projection={actual}");
    }
    let expected: Value = serde_json::from_str(include_str!(
        "accounting/fixtures/uncapped-gmin4-direct.json"
    ))
    .unwrap();
    assert_eq!(actual, expected["projection"]);
    for field in ["payloads", "stderr", "records"] {
        let mut mutated = actual.clone();
        mutated[field][0] = json!(format!("{}!", mutated[field][0].as_str().unwrap()));
        assert_ne!(mutated, expected["projection"]);
    }
    let mut mutated = actual.clone();
    mutated["records"][0] = json!(mutated["records"][0]
        .as_str()
        .unwrap()
        .replace("\"dispatched\":true", "\"dispatched\":false"));
    assert_ne!(mutated, expected["projection"]);
    let mut mutated = actual.clone();
    mutated["records"].as_array_mut().unwrap().pop();
    assert_ne!(mutated, expected["projection"]);
}
