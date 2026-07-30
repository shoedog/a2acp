use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn mcp_subcommand_handshake_and_tool_call_over_real_pipes() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("a2a-bridge.toml");
    let store_path = dir.path().join("tasks.sqlite");
    std::fs::write(
        &cfg_path,
        format!(
            "default = \"codex\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [store]\npath = {store:?}\n\n\
             [[agents]]\nid = \"codex\"\ncmd = \"codex\"\nkind = \"acp\"\n",
            store = store_path,
        ),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&cfg_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    let reqs = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"status","arguments":{"task_id":"task-nope"}}}"#,
    ];
    for r in reqs {
        stdin.write_all(r.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();
    drop(stdin);

    let mut out = String::new();
    tokio::time::timeout(Duration::from_secs(30), stdout.read_to_string(&mut out))
        .await
        .expect("timed out reading mcp stdout")
        .unwrap();
    let frames: Vec<serde_json::Value> = out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    assert_eq!(frames.len(), 3, "frames: {out}");
    assert_eq!(frames[0]["id"], 1);
    assert_eq!(frames[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(frames[1]["id"], 2);
    // Slice 9 added the `inject` + `permit` tools (6 -> 8).
    assert_eq!(frames[1]["result"]["tools"].as_array().unwrap().len(), 8);
    assert_eq!(frames[2]["id"], 3);
    assert_eq!(frames[2]["result"]["isError"], true);

    let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
        .await
        .expect("timed out waiting for mcp child")
        .unwrap();
    assert!(status.success(), "mcp child exited nonzero on EOF");
}

#[tokio::test]
async fn framed_mcp_prompt_barrier_refuses_provider_and_terminalizes_once() {
    let provider = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("a2a-bridge.toml");
    let store_path = dir.path().join("owner.sqlite");
    drop(bridge_store::sqlite::SqliteStore::open_shared_history(&store_path).unwrap());

    let fault = rusqlite::Connection::open(&store_path).unwrap();
    fault
        .execute_batch(
            "CREATE TABLE mcp_barrier_audit (
                 terminal_writes INTEGER NOT NULL
             );
             INSERT INTO mcp_barrier_audit VALUES (0);
             CREATE TRIGGER fail_mcp_prompt_barrier
             BEFORE UPDATE OF prompt_acceptance ON workflow_attempt_summaries
             WHEN NEW.prompt_acceptance = 'dispatch_uncertain'
             BEGIN
                 SELECT RAISE(ABORT, 'injected MCP prompt barrier failure');
             END;
             CREATE TRIGGER count_mcp_terminal_write
             AFTER UPDATE OF terminal_json ON workflow_attempt_summaries
             WHEN OLD.terminal_json IS NULL AND NEW.terminal_json IS NOT NULL
             BEGIN
                 UPDATE mcp_barrier_audit
                 SET terminal_writes = terminal_writes + 1;
             END;",
        )
        .unwrap();
    drop(fault);

    std::fs::write(
        &cfg_path,
        format!(
            "default = \"api\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [store]\npath = {store:?}\n\n\
             [[agents]]\nid = \"api\"\nkind = \"api\"\nbase_url = {base_url:?}\n\
             api_key_env = \"A2A_BRIDGE_TEST_API_KEY\"\nmodel = \"fake-model\"\n",
            store = store_path,
            base_url = format!("{}/v1", provider.uri()),
        ),
    )
    .unwrap();

    let mut owner = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&cfg_path)
        .env("A2A_BRIDGE_TEST_API_KEY", "test-only")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut owner_stdin = owner.stdin.take().unwrap();
    let mut owner_stdout = BufReader::new(owner.stdout.take().unwrap()).lines();
    owner_stdin
        .write_all(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}
"#,
        )
        .await
        .unwrap();
    owner_stdin.flush().await.unwrap();
    let initialized = tokio::time::timeout(Duration::from_secs(30), owner_stdout.next_line())
        .await
        .expect("MCP owner did not initialize")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&initialized).unwrap()["id"],
        1
    );
    owner_stdin
        .write_all(
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}
"#,
        )
        .await
        .unwrap();

    let attempt_id = "attempt-44444444444444444444444444444444";
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run",
            "arguments": {
                "input": "must not reach provider",
                "agent": "api",
                "execution_id": "exec-33333333333333333333333333333333",
                "attempt_id": attempt_id,
            }
        }
    });
    owner_stdin
        .write_all(format!("{call}\n").as_bytes())
        .await
        .unwrap();
    owner_stdin.flush().await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(30), owner_stdout.next_line())
        .await
        .expect("MCP prompt barrier did not return")
        .unwrap()
        .unwrap();
    let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["id"], 2);
    assert_eq!(reply["result"]["isError"], true);
    assert!(provider.received_requests().await.unwrap().is_empty());

    drop(owner_stdin);
    let status = tokio::time::timeout(Duration::from_secs(30), owner.wait())
        .await
        .expect("MCP owner did not stop after EOF")
        .unwrap();
    assert!(status.success());

    let history = bridge_store::sqlite::SqliteStore::open_history_read_only(&store_path).unwrap();
    let attempt_id = bridge_core::ids::AttemptId::parse(attempt_id).unwrap();
    let record =
        bridge_core::workflow_history::WorkflowHistoryStore::attempt(&history, &attempt_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        record.reservation.surface,
        bridge_core::workflow_history::ExecutionSurface::Mcp
    );
    let terminal = record.terminal.unwrap();
    assert_eq!(terminal.terminal_reason, "prompt_barrier_failed");
    assert_eq!(terminal.prompt_acceptance, "unknown");
    assert_eq!(terminal.cleanup_disposition, "complete");
    assert!(terminal.degraded);
    assert!(!terminal.telemetry_complete);

    let audit = rusqlite::Connection::open(&store_path).unwrap();
    let counts: (i64, i64) = audit
        .query_row(
            "SELECT terminal_writes,
                    (SELECT COUNT(*) FROM workflow_attempt_summaries)
             FROM mcp_barrier_audit",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(counts, (1, 1));
}

#[tokio::test]
async fn workflow_stats_get_reads_live_mcp_owner_active_and_terminal_wal_rows() {
    let provider = MockServer::start().await;
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"PONG\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse)
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&provider)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("a2a-bridge.toml");
    let execution_id = "exec-11111111111111111111111111111111";
    let attempt_id = "attempt-22222222222222222222222222222222";
    let store_override = std::path::PathBuf::from("owner-data/owner.sqlite");
    let store_path = dir.path().join(&store_override);
    let configured_store = std::path::PathBuf::from("configured-data/config.sqlite");
    let configured_store_path = dir.path().join(&configured_store);
    std::fs::create_dir_all(store_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(configured_store_path.parent().unwrap()).unwrap();
    let reader_cwd = dir.path().join("reader-cwd");
    let wrong_reader_store = reader_cwd.join(&store_override);
    std::fs::create_dir_all(wrong_reader_store.parent().unwrap()).unwrap();
    let wrong_reader =
        bridge_store::sqlite::SqliteStore::open_shared_history(&wrong_reader_store).unwrap();
    let wrong_identity = bridge_core::ids::AttemptIdentity {
        execution_id: bridge_core::ids::ExecutionId::parse(execution_id).unwrap(),
        attempt_id: bridge_core::ids::AttemptId::parse(attempt_id).unwrap(),
        ordinal: 0,
        parent_attempt_id: None,
    };
    bridge_core::workflow_history::WorkflowHistoryStore::reserve(
        &wrong_reader,
        &bridge_core::workflow_history::AttemptReservation {
            identity: wrong_identity,
            task_id: Some(bridge_core::ids::TaskId::parse(execution_id).unwrap()),
            workflow: "wrong-reader".into(),
            task_class: "other".into(),
            surface: bridge_core::workflow_history::ExecutionSurface::Mcp,
            policy: "r2f0a".into(),
            workload_fingerprint: "shape-wrong-reader".into(),
            started_ms: 1,
            workload_fingerprint_complete: true,
            prompt_acceptance: "not_dispatched".into(),
            pinned: false,
        },
    )
    .await
    .unwrap();
    drop(wrong_reader);
    std::fs::write(
        &cfg_path,
        format!(
            "default = \"api\"\n\n\
             [server]\naddr = \"127.0.0.1:0\"\n\n\
             [store]\npath = {store:?}\n\n\
             [[agents]]\nid = \"api\"\nkind = \"api\"\nbase_url = {base_url:?}\n\
             api_key_env = \"A2A_BRIDGE_TEST_API_KEY\"\nmodel = \"fake-model\"\n",
            store = configured_store,
            base_url = format!("{}/v1", provider.uri()),
        ),
    )
    .unwrap();

    #[cfg(unix)]
    let stats_config_path = {
        use std::os::unix::fs::symlink;

        let alias_directory = dir.path().join("config-alias");
        std::fs::create_dir(&alias_directory).unwrap();
        let alias = alias_directory.join("a2a-bridge.toml");
        symlink(&cfg_path, &alias).unwrap();
        alias
    };
    #[cfg(not(unix))]
    let stats_config_path = cfg_path.clone();

    // Initialize both complete schemas. The configured override must preserve the
    // caller's WAL policy rather than installing the platform rollback journal.
    drop(bridge_store::sqlite::SqliteStore::open(&configured_store_path).unwrap());
    drop(bridge_store::sqlite::SqliteStore::open(&store_path).unwrap());
    let canonical_store_path = std::fs::canonicalize(&store_path).unwrap();

    let mut owner = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&cfg_path)
        .arg("--store")
        .arg(&store_override)
        .current_dir(&reader_cwd)
        .env("A2A_BRIDGE_TEST_API_KEY", "test-only")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut owner_stdin = owner.stdin.take().unwrap();
    let mut owner_stdout = BufReader::new(owner.stdout.take().unwrap()).lines();

    owner_stdin
        .write_all(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}
"#,
        )
        .await
        .unwrap();
    owner_stdin.flush().await.unwrap();
    let initialized = tokio::time::timeout(Duration::from_secs(30), owner_stdout.next_line())
        .await
        .expect("MCP owner did not initialize")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&initialized).unwrap()["id"],
        1
    );
    owner_stdin
        .write_all(
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}
"#,
        )
        .await
        .unwrap();

    let mut competing_command = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
    competing_command
        .arg("mcp")
        .arg("--config")
        .arg(&cfg_path)
        .arg("--store")
        .arg(&store_override)
        .current_dir(&reader_cwd)
        .env("A2A_BRIDGE_TEST_API_KEY", "test-only")
        .kill_on_drop(true);
    let competing = tokio::time::timeout(Duration::from_secs(10), competing_command.output())
        .await
        .expect("competing writable owner did not refuse")
        .unwrap();
    assert!(!competing.status.success());
    assert!(
        String::from_utf8_lossy(&competing.stderr).contains("Locked"),
        "unexpected writable-owner failure: {}",
        String::from_utf8_lossy(&competing.stderr)
    );

    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "run",
            "arguments": {
                "input": "Reply PONG",
                "agent": "api",
                "execution_id": execution_id,
                "attempt_id": attempt_id,
            }
        }
    });
    owner_stdin
        .write_all(format!("{call}\n").as_bytes())
        .await
        .unwrap();
    owner_stdin.flush().await.unwrap();

    let mut active = None;
    let mut last_reader_error = String::new();
    let mut last_reader_row = serde_json::Value::Null;
    for _ in 0..80 {
        let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .args(["workflow-stats", "get", attempt_id, "--store"])
            .arg(&canonical_store_path)
            .arg("--json")
            .output()
            .await
            .unwrap();
        if output.status.success() {
            let row: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            last_reader_row = row.clone();
            if row["terminal"].is_null()
                && row["reservation"]["prompt_acceptance"] == "dispatch_uncertain"
            {
                active = Some(row);
                break;
            }
        } else {
            last_reader_error = String::from_utf8_lossy(&output.stderr).into_owned();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let active = active.unwrap_or_else(|| {
        panic!("read-only CLI never observed the active WAL row: {last_reader_error}; last row: {last_reader_row}")
    });
    assert!(
        owner.try_wait().unwrap().is_none(),
        "MCP lifetime owner exited before the active read"
    );
    assert_eq!(active["reservation"]["surface"], "mcp");
    assert_eq!(active["reservation"]["identity"]["attempt_id"], attempt_id);

    let configured_reader = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["workflow-stats", "get", attempt_id, "--config"])
        .arg(&stats_config_path)
        .arg("--json")
        .current_dir(&reader_cwd)
        .output()
        .await
        .unwrap();
    assert!(!configured_reader.status.success());
    assert!(
        String::from_utf8_lossy(&configured_reader.stderr).contains("Error: AttemptNotFound"),
        "configured store unexpectedly contained the override owner's attempt: {}",
        String::from_utf8_lossy(&configured_reader.stderr)
    );

    let relative_reader = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["workflow-stats", "get", attempt_id, "--store"])
        .arg(&store_override)
        .arg("--json")
        .current_dir(&reader_cwd)
        .output()
        .await
        .unwrap();
    assert!(!relative_reader.status.success());
    assert!(
        String::from_utf8_lossy(&relative_reader.stderr).contains("Error: StorePathNotAbsolute"),
        "relative reader unexpectedly consulted its cwd store: {}",
        String::from_utf8_lossy(&relative_reader.stderr)
    );

    // Production ownership uses TRUNCATE for the aggregate-size invariant. Switch
    // this isolated fixture back to WAL only after the MCP-owned active row exists,
    // then pin an older reader snapshot so the CLI must consume a newer WAL frame.
    let wal_writer = rusqlite::Connection::open(&store_path).unwrap();
    wal_writer
        .execute_batch("PRAGMA busy_timeout = 5000;")
        .unwrap();
    let owner_mode: String = wal_writer
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        owner_mode, "wal",
        "configured MCP ownership changed the caller's journal policy"
    );
    let wal_mode: String = wal_writer
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .unwrap();
    assert_eq!(wal_mode, "wal");

    let active_snapshot = rusqlite::Connection::open(&store_path).unwrap();
    active_snapshot.execute_batch("BEGIN").unwrap();
    let snapshot_pinned: i64 = active_snapshot
        .query_row(
            "SELECT pinned FROM workflow_attempt_summaries WHERE attempt_id=?1",
            [attempt_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(snapshot_pinned, 0);

    let mut wal_reservation = active["reservation"].clone();
    wal_reservation["pinned"] = serde_json::Value::Bool(true);
    wal_writer
        .execute(
            "UPDATE workflow_attempt_summaries
             SET pinned=1, reservation_json=?2 WHERE attempt_id=?1",
            rusqlite::params![attempt_id, serde_json::to_string(&wal_reservation).unwrap()],
        )
        .unwrap();
    let mut wal_name = store_path.as_os_str().to_os_string();
    wal_name.push("-wal");
    let wal_path = std::path::PathBuf::from(wal_name);
    assert!(std::fs::metadata(&wal_path).unwrap().len() > 0);

    let wal_active = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["workflow-stats", "get", attempt_id, "--store"])
        .arg(&canonical_store_path)
        .arg("--json")
        .output()
        .await
        .unwrap();
    assert!(
        wal_active.status.success(),
        "{}",
        String::from_utf8_lossy(&wal_active.stderr)
    );
    let wal_active: serde_json::Value = serde_json::from_slice(&wal_active.stdout).unwrap();
    assert_eq!(wal_active["reservation"]["pinned"], true);
    assert_eq!(
        active_snapshot
            .query_row(
                "SELECT pinned FROM workflow_attempt_summaries WHERE attempt_id=?1",
                [attempt_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0,
        "the pinned snapshot must not see the newer WAL frame"
    );
    active_snapshot.execute_batch("ROLLBACK").unwrap();

    wal_reservation["pinned"] = serde_json::Value::Bool(false);
    wal_writer
        .execute(
            "UPDATE workflow_attempt_summaries
             SET pinned=0, reservation_json=?2 WHERE attempt_id=?1",
            rusqlite::params![attempt_id, serde_json::to_string(&wal_reservation).unwrap()],
        )
        .unwrap();
    let (busy, _, _): (i64, i64, i64) = wal_writer
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(busy, 0);

    // Start another old snapshot with an empty WAL. The owner's strict preflight
    // can pass, but its terminal commit cannot be checkpointed past this reader.
    let terminal_snapshot = rusqlite::Connection::open(&store_path).unwrap();
    terminal_snapshot.execute_batch("BEGIN").unwrap();
    let snapshot_terminal: Option<String> = terminal_snapshot
        .query_row(
            "SELECT terminal_json FROM workflow_attempt_summaries WHERE attempt_id=?1",
            [attempt_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(snapshot_terminal.is_none());

    let reply = tokio::time::timeout(Duration::from_secs(30), owner_stdout.next_line())
        .await
        .expect("MCP prompt did not complete")
        .unwrap()
        .unwrap();
    let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["id"], 2);
    let result = reply["result"]
        .as_object()
        .expect("successful MCP reply must contain a result object");
    assert!(
        !result.contains_key("isError"),
        "successful MCP result unexpectedly contains isError: {reply}"
    );

    let snapshot_terminal: Option<String> = terminal_snapshot
        .query_row(
            "SELECT terminal_json FROM workflow_attempt_summaries WHERE attempt_id=?1",
            [attempt_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        snapshot_terminal.is_none(),
        "the old snapshot must not see the owner's terminal WAL commit"
    );
    assert!(
        std::fs::metadata(&wal_path).unwrap().len() > 0,
        "the terminal commit must still be present in the WAL"
    );

    let terminal = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["workflow-stats", "get", attempt_id, "--store"])
        .arg(&canonical_store_path)
        .arg("--json")
        .output()
        .await
        .unwrap();
    assert!(
        terminal.status.success(),
        "{}",
        String::from_utf8_lossy(&terminal.stderr)
    );
    let terminal: serde_json::Value = serde_json::from_slice(&terminal.stdout).unwrap();
    assert_eq!(terminal["terminal"]["outcome"], "completed");
    assert_eq!(
        terminal["terminal"]["prompt_acceptance"],
        "dispatch_uncertain"
    );
    assert!(
        owner.try_wait().unwrap().is_none(),
        "MCP lifetime owner exited before the terminal read"
    );

    terminal_snapshot.execute_batch("ROLLBACK").unwrap();
    drop(owner_stdin);
    let status = tokio::time::timeout(Duration::from_secs(30), owner.wait())
        .await
        .expect("MCP owner did not stop after EOF")
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn managed_agent_depth_refuses_before_config_or_store_work() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&missing)
        .env("A2A_BRIDGE_MCP_CALL_DEPTH", "1")
        .output()
        .await
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("managed-agent MCP loopback is unsupported"),
        "guard must own the failure before config resolution: {stderr}"
    );
    assert!(
        !missing.exists(),
        "guard must not create the missing config"
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "guard must create no config, store, lease, or other artifact"
    );
}

#[tokio::test]
async fn external_depth_zero_keeps_existing_mcp_startup_path() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&missing)
        .env("A2A_BRIDGE_MCP_CALL_DEPTH", "0")
        .output()
        .await
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("run `a2a-bridge init`"), "got: {stderr}");
    assert!(
        !stderr.contains("managed-agent MCP loopback is unsupported"),
        "depth zero is the supported external-controller path: {stderr}"
    );
}

#[tokio::test]
async fn malformed_managed_depth_fails_closed_before_config_work() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&missing)
        .env("A2A_BRIDGE_MCP_CALL_DEPTH", "not-a-depth")
        .output()
        .await
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid A2A_BRIDGE_MCP_CALL_DEPTH"),
        "malformed lineage must fail closed: {stderr}"
    );
    assert!(!missing.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn non_unicode_managed_depth_fails_closed_before_config_work() {
    use std::os::unix::ffi::OsStringExt;

    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--config")
        .arg(&missing)
        .env(
            "A2A_BRIDGE_MCP_CALL_DEPTH",
            std::ffi::OsString::from_vec(vec![0xff]),
        )
        .output()
        .await
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid non-Unicode A2A_BRIDGE_MCP_CALL_DEPTH"),
        "non-Unicode lineage must fail closed: {stderr}"
    );
    assert!(!missing.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn managed_depth_does_not_hide_side_effect_free_mcp_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("mcp")
        .arg("--help")
        .env("A2A_BRIDGE_MCP_CALL_DEPTH", "1")
        .output()
        .await
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("usage: a2a-bridge mcp"), "got: {stdout}");
    assert!(stdout.contains("external-controller MCP"), "got: {stdout}");
    assert!(
        stdout.contains("Managed-agent loopback is refused"),
        "got: {stdout}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("managed-agent MCP loopback"),
        "help returns before the runtime loopback guard"
    );
}
