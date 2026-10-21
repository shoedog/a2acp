//! Real compatibility CLI -> staged candidate -> smoke -> fixture ACP accounting.
#![cfg(unix)]
use bridge_core::{accounting::*, task_store::TaskStore};
use bridge_store::sqlite::SqliteStore;
use serde_json::{json, Value};
use std::{fs, os::unix::fs::symlink, path::PathBuf, process::Command, time::Instant};
#[allow(dead_code)] // The shared fixture also has a standalone-smoke command builder.
#[path = "accounting/smoke_support.rs"]
mod smoke_support;
mod text_file_busy;
use smoke_support::SmokeCli;

struct CompatibilityCli {
    smoke: SmokeCli,
    manifest: PathBuf,
    output_dir: PathBuf,
}
impl CompatibilityCli {
    fn new(configured: bool) -> Self {
        let smoke = SmokeCli::new(configured, false);
        // Metadata describes only the checked-in shell fixture; no installed provider is used.
        fs::write(
            smoke.dir.path().join("package.json"),
            json!({
                "name": "@agentclientprotocol/codex-acp", "version": "1.2.3",
                "bin": {"fixture": "fixture-acp"}
            })
            .to_string(),
        )
        .unwrap();
        let cli_package = smoke.dir.path().join("node_modules/@openai/codex");
        fs::create_dir_all(&cli_package).unwrap();
        fs::write(
            cli_package.join("package.json"),
            json!({
                "name": "@openai/codex", "version": "4.5.6"
            })
            .to_string(),
        )
        .unwrap();
        let output_dir = smoke.dir.path().join("evidence");
        fs::create_dir(&output_dir).unwrap();
        let manifest = smoke.dir.path().join("manifest.toml");
        let result = Self {
            smoke,
            manifest,
            output_dir,
        };
        result.write_manifest();
        result
    }
    fn write_manifest(&self) {
        let digest = ring::digest::digest(
            &ring::digest::SHA256,
            &fs::read(&self.smoke.config).unwrap(),
        );
        let hash: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        fs::write(
            &self.manifest,
            format!(
                r#"schema_version = 1
[budget]
timeout_secs = 30
max_tokens = 1000
max_cost_usd = 1.0
[[cases]]
id = "fixture"
lane = "pinned"
evidence_path = "bridge_smoke"
execution_mode = "host"
os = {os:?}
architecture = {arch:?}
environment_owner = "accounting-fixture"
config = "a2a-bridge.toml"
agent = "agent"
model = "good"
auth_path = "pre_authenticated"
required_env = []
probe = "minimal"
billable = true
timeout_secs = 5
max_tokens = 100
max_cost_usd = 0.5
retry_cap = 0
expected_status = "PASS"
classification = "support"
[cases.artifact]
retention_days = 1
redaction = "strict"
[cases.pins]
config_sha256 = {hash:?}
model = "good"
adapter = "@agentclientprotocol/codex-acp=1.2.3"
agent_cli = "@openai/codex=4.5.6"
"#,
                os = std::env::consts::OS,
                arch = std::env::consts::ARCH
            ),
        )
        .unwrap();
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
        cmd.args(["compatibility", "run", "--manifest"])
            .arg(&self.manifest)
            .args([
                "--case",
                "fixture",
                "--environment-owner",
                "accounting-fixture",
                "--out",
            ])
            .arg(self.output_dir.join("aggregate.json"))
            .env("A2A_BRIDGE_STATE_DIR", &self.smoke.platform)
            .env("A2A_LEASE_DIR", self.smoke.dir.path().join("leases"))
            .current_dir(self.smoke.dir.path());
        cmd
    }
    fn prepend_gate(&self) -> (PathBuf, fs::File) {
        use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
        let ready = self.smoke.dir.path().join("gate-ready");
        let fifo = self.smoke.dir.path().join("gate-release");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a live NUL-terminated path; the mode creates an owner-only FIFO.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        // Keep a reader/writer open until the runner exits so release cannot block or get lost.
        let release = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo)
            .unwrap();
        let adapter = self.smoke.dir.path().join("gate-agent");
        fs::write(
            &adapter,
            r#"#!/bin/sh
[ "$#" -eq 2 ] || exit 0
IFS= read -r line
id=$(printf '%s\n' "$line" | sed -E 's/^\{"jsonrpc":"2.0","id":([^,]+),"method".*/\1/')
printf ready > "$1"
IFS= read -r release < "$2"
printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"fixture gate released"}}\n' "$id"
"#,
        )
        .unwrap();
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
        text_file_busy::warm_up_script(&adapter);
        let package_path = self.smoke.dir.path().join("package.json");
        let mut package: Value = serde_json::from_slice(&fs::read(&package_path).unwrap()).unwrap();
        package["bin"]["gate"] = json!("gate-agent");
        fs::write(package_path, package.to_string()).unwrap();
        let mut config: toml::Value = fs::read_to_string(&self.smoke.config)
            .unwrap()
            .parse()
            .unwrap();
        config.as_table_mut().unwrap().insert(
            "store".into(),
            toml::Value::try_from(json!({"path": "gate.sqlite"})).unwrap(),
        );
        config["registry"]["allowed_cmds"] = toml::Value::try_from(json!([adapter])).unwrap();
        config["agents"][0]["cmd"] = adapter.to_str().unwrap().into();
        config["agents"][0]["args"] = toml::Value::try_from(json!([ready, fifo])).unwrap();
        let path = self.smoke.dir.path().join("gate.toml");
        fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        let mut manifest: toml::Value =
            fs::read_to_string(&self.manifest).unwrap().parse().unwrap();
        let mut gate = manifest["cases"][0].clone();
        gate["id"] = "gate".into();
        gate["config"] = "gate.toml".into();
        gate["expected_status"] = "FAIL".into();
        let digest = ring::digest::digest(&ring::digest::SHA256, &fs::read(path).unwrap());
        gate["pins"]["config_sha256"] = digest
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            .into();
        manifest["cases"].as_array_mut().unwrap().insert(0, gate);
        fs::write(&self.manifest, toml::to_string(&manifest).unwrap()).unwrap();
        (ready, release)
    }
    fn aggregate(&self) -> Value {
        serde_json::from_slice(&fs::read(self.output_dir.join("aggregate.json")).unwrap()).unwrap()
    }
    fn assert_cleaned(&self) {
        let entries: Vec<_> = fs::read_dir(&self.output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(
            entries,
            vec!["aggregate.json"],
            "scratch and child artifact must be removed"
        );
    }
}

#[tokio::test]
async fn staged_child_usage_survives_cleanup_and_aggregate_rereads() {
    for configured in [true, false] {
        let cli = CompatibilityCli::new(configured);
        let output = cli
            .command()
            .arg("--acknowledge-billable")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            cli.aggregate()
        );
        cli.assert_cleaned();
        let aggregate = cli.aggregate();
        assert_eq!(aggregate["success"], true);
        assert_eq!(
            aggregate["candidate"]["byte_length"],
            fs::metadata(env!("CARGO_BIN_EXE_a2a-bridge"))
                .unwrap()
                .len()
        );
        assert_eq!(aggregate["budget"]["observed_tokens"], 15);
        assert_eq!(aggregate["budget"]["observed_cost_usd"], 0.25);
        let smoke = &aggregate["results"][0]["smoke"];
        assert_eq!(smoke["success"], true);
        assert_eq!(smoke["turn"]["prompt_calls"], 1);
        let prompts = cli.smoke.prompts();
        assert_eq!(prompts.len(), 1);
        assert_eq!(
            prompts[0]["params"]["prompt"][0]["text"],
            "Reply exactly PONG. Do not use tools."
        );
        let rows = cli.smoke.rows();
        assert_eq!(
            rows.len(),
            1,
            "aggregate must not charge its child's usage again"
        );
        let row = &rows[0];
        assert_eq!(
            row.attribution.execution.as_deref(),
            smoke["attempt"]["execution_id"].as_str()
        );
        assert_eq!(
            row.attribution.run_attempt.as_deref(),
            smoke["attempt"]["attempt_id"].as_str()
        );
        assert_eq!(row.attribution.turn.as_ref(), Some(&row.id));
        assert_eq!(row.attribution.agent, "agent");
        assert_eq!(row.attribution.effective_model.as_deref(), Some("good"));
        assert_eq!(row.attribution.attempt_kind, "smoke");
        assert!(row.dispatched && row.finalized);
        assert_eq!(
            serde_json::to_value(&row.usage).unwrap(),
            smoke["turn"]["usage"]
        );
        let store = SqliteStore::open_history_read_only(&cli.smoke.store).unwrap();
        let query = UsageQueryV1 {
            execution: row.attribution.execution.clone(),
            run_attempt: row.attribution.run_attempt.clone(),
            ..Default::default()
        };
        let summary = store.usage_summary_v1(&query).await.unwrap();
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
        assert_eq!(
            (
                bucket.thought_tokens.reported_sum,
                bucket.cached_read_tokens.reported_sum,
                bucket.cached_write_tokens.reported_sum
            ),
            (2, 3, 4)
        );
        assert_eq!(bucket.cost_by_currency["USD"], 0.25);
        assert_eq!((row.usage.used, row.usage.size), (Some(99), Some(1000)));
        drop(store);
        // Exercise production aggregate parsing/processing again, without running a prompt.
        let reread = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .args(["compatibility", "compare", "--current"])
            .arg(cli.output_dir.join("aggregate.json"))
            .arg("--baseline")
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../compatibility/baselines/pinned.json"),
            )
            .env("A2A_BRIDGE_STATE_DIR", &cli.smoke.platform)
            .output()
            .unwrap();
        let comparison: Value = serde_json::from_slice(&reread.stdout).unwrap();
        assert_eq!(comparison["schema_version"], 1);
        assert_eq!(
            comparison["equal"], false,
            "fixture differs from production baseline"
        );
        assert_eq!(cli.aggregate(), aggregate);
        assert_eq!(
            serde_json::to_value(cli.smoke.rows()).unwrap(),
            serde_json::to_value(rows).unwrap()
        );
        let reopened = SqliteStore::open_history_read_only(&cli.smoke.store).unwrap();
        assert_eq!(reopened.usage_summary_v1(&query).await.unwrap(), summary);
    }
}

#[tokio::test]
async fn child_writer_refusal_preserves_failure_and_never_prompts() {
    let cli = CompatibilityCli::new(true);
    let store = SqliteStore::open_shared_history(&cli.smoke.store).unwrap();
    store.usage_summary_v1(&Default::default()).await.unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(cli.smoke.store.with_extension("sqlite.usage-v1.sqlite"))
        .unwrap();
    conn.execute_batch("CREATE TRIGGER refuse_usage BEFORE INSERT ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'fixture write refusal'); END;").unwrap();
    drop(conn);
    let output = cli
        .command()
        .arg("--acknowledge-billable")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let aggregate = cli.aggregate();
    let case = &aggregate["results"][0];
    assert_eq!(case["execution"], "completed");
    assert_eq!(case["actual_status"], "FAIL");
    assert!(
        case["runner_error_code"].is_null(),
        "child failure must remain child evidence"
    );
    assert_eq!(case["smoke"]["turn"]["prompt_calls"], 0);
    assert_eq!(
        case["smoke"]["attempt"]["prompt_may_have_been_accepted"],
        false
    );
    assert_eq!(
        case["smoke"]["diagnostics"]["failure"]["code"],
        "smoke.durable_evidence_unavailable"
    );
    assert_eq!(aggregate["budget"]["observed_tokens"], 0);
    assert_eq!(aggregate["budget"]["observed_cost_usd"], 0.0);
    assert!(cli.smoke.prompts().is_empty());
    assert!(cli.smoke.rows().is_empty());
    cli.assert_cleaned();
}

#[cfg(target_os = "linux")]
#[test]
fn inaccessible_external_store_retains_child_failure_and_continues() {
    use std::os::unix::process::CommandExt;
    // The container runs tests as root. Re-exec this test as daemon (whose passwd home exists)
    // so the real compatibility parent AND its staged smoke child observe EACCES.
    // SAFETY: geteuid only reads the process credentials.
    if unsafe { libc::geteuid() } == 0 {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "inaccessible_external_store_retains_child_failure_and_continues",
                "--nocapture",
            ])
            .current_dir("/tmp")
            .uid(1)
            .gid(1)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains("running 1 test"));
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    assert_external_store_failure_continues(false);
}

#[test]
fn external_store_beneath_regular_file_retains_child_failure_and_continues() {
    assert_external_store_failure_continues(true);
}

fn assert_external_store_failure_continues(regular_file: bool) {
    use std::os::unix::fs::PermissionsExt;

    for configured in [true, false] {
        let mut cli = CompatibilityCli::new(configured);
        let blocked = cli.smoke.dir.path().join("blocked");
        if regular_file {
            fs::write(&blocked, b"not a directory").unwrap();
        } else {
            fs::create_dir(&blocked).unwrap();
        }
        let mut valid: toml::Value = fs::read_to_string(&cli.smoke.config)
            .unwrap()
            .parse()
            .unwrap();
        valid.as_table_mut().unwrap().insert(
            "store".into(),
            toml::Value::try_from(json!({"path": "tasks.sqlite"})).unwrap(),
        );
        let valid_config = cli.smoke.dir.path().join("valid.toml");
        fs::write(&valid_config, toml::to_string(&valid).unwrap()).unwrap();
        if configured {
            valid["store"]["path"] = "blocked/tasks.sqlite".into();
            fs::write(&cli.smoke.config, toml::to_string(&valid).unwrap()).unwrap();
        }
        cli.write_manifest();
        let mut manifest: toml::Value = fs::read_to_string(&cli.manifest).unwrap().parse().unwrap();
        let mut later = manifest["cases"][0].clone();
        later["id"] = "later".into();
        later["config"] = "valid.toml".into();
        let digest = ring::digest::digest(&ring::digest::SHA256, &fs::read(&valid_config).unwrap());
        later["pins"]["config_sha256"] = digest
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            .into();
        manifest["cases"].as_array_mut().unwrap().push(later);
        fs::write(&cli.manifest, toml::to_string(&manifest).unwrap()).unwrap();
        cli.smoke.store = cli.smoke.dir.path().join("tasks.sqlite");
        let mut command = cli.command();
        command.args(["--case", "later", "--acknowledge-billable"]);
        if !configured {
            command.env("A2A_BRIDGE_STATE_DIR", &blocked);
        }
        if regular_file {
            assert_eq!(
                fs::metadata(blocked.join("tasks.sqlite"))
                    .unwrap_err()
                    .raw_os_error(),
                Some(libc::ENOTDIR)
            );
        } else {
            fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
            assert_eq!(
                fs::metadata(blocked.join("tasks.sqlite"))
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }
        let output = command.output().unwrap();
        if !regular_file {
            fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(!output.status.success()); // The first case failed, despite the later success.
        let aggregate = cli.aggregate();
        let first = &aggregate["results"][0];
        assert_eq!(first["execution"], "completed", "{aggregate}");
        assert_eq!(first["actual_status"], "FAIL");
        assert!(first["runner_error_code"].is_null());
        assert_eq!(
            first["smoke"]["diagnostics"]["failure"]["code"],
            "smoke.durable_evidence_unavailable"
        );
        assert_eq!(first["smoke"]["turn"]["prompt_calls"], 0);
        assert!(first["smoke"]["turn"]["usage"].is_null());
        assert_eq!(
            first["smoke"]["attempt"]["prompt_may_have_been_accepted"],
            false
        );
        if regular_file {
            assert_eq!(fs::read(&blocked).unwrap(), b"not a directory");
        } else {
            assert_eq!(fs::read_dir(&blocked).unwrap().count(), 0);
        }
        let later = &aggregate["results"][1]["smoke"];
        assert_eq!(later["success"], true, "{aggregate}");
        assert_eq!(cli.smoke.prompts().len(), 1);
        let rows = cli.smoke.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].attribution.run_attempt.as_deref(),
            later["attempt"]["attempt_id"].as_str()
        );
        assert_eq!(aggregate["budget"]["observed_tokens"], 15);
        assert_eq!(aggregate["budget"]["observed_cost_usd"], 0.25);
        cli.assert_cleaned();
    }
}

#[test]
fn scratch_accounting_destinations_refuse_before_provider_prompting() {
    // The config-alias case selects a direct relative store under the canonical config's
    // scratch parent. The directory-alias cases keep the config outside scratch.
    for route in ["config-alias", "directory-alias", "platform-alias"] {
        let cli = CompatibilityCli::new(route != "platform-alias");
        let alias = cli.smoke.dir.path().join("store-alias");
        let config_bytes = fs::read(&cli.smoke.config).unwrap();
        if route == "directory-alias" {
            let raw = String::from_utf8(config_bytes.clone())
                .unwrap()
                .replace("tasks.sqlite", "store-alias/tasks.sqlite");
            fs::write(&cli.smoke.config, raw).unwrap();
            cli.write_manifest();
        }
        if route == "config-alias" {
            let raw = fs::read_to_string(&cli.manifest).unwrap().replace(
                "config = \"a2a-bridge.toml\"",
                "config = \"store-alias/config.toml\"",
            );
            fs::write(&cli.manifest, raw).unwrap();
        }
        let (ready, mut release) = cli.prepend_gate();
        let mut command = cli.command();
        command.args(["--case", "gate"]);
        if route == "platform-alias" {
            command.env("A2A_BRIDGE_STATE_DIR", &alias);
        }
        let mut child = command
            .arg("--acknowledge-billable")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // The first real smoke child waits before its fixture replies to initialize. The
        // target case cannot be admitted until setup finishes and we release that child.
        let started = Instant::now();
        while !ready.exists() {
            assert!(
                started.elapsed().as_secs() < 20,
                "gate not reached: {route}"
            );
            assert!(
                child.try_wait().unwrap().is_none(),
                "runner exited before gate"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        std::thread::sleep(std::time::Duration::from_millis(250)); // Deliberately delayed setup.
        let scratch = fs::read_dir(&cli.output_dir)
            .unwrap()
            .flatten()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".a2a-compat-")
            })
            .expect("the gate holds the real scratch directory alive")
            .path();
        if route == "config-alias" {
            fs::write(scratch.join("config.toml"), config_bytes).unwrap();
            symlink(&scratch, &alias).unwrap();
        } else {
            symlink(&scratch, &alias).unwrap();
        }
        std::io::Write::write_all(&mut release, b"go\n").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            !output.status.success(),
            "disposable accounting must refuse: {route}"
        );
        assert!(
            cli.smoke.prompts().is_empty(),
            "provider was prompted with disposable accounting: {route}"
        );
        let aggregate = cli.aggregate();
        assert_eq!(aggregate["results"][0]["case_id"], "gate");
        assert_eq!(aggregate["results"][0]["execution"], "completed");
        assert_eq!(
            aggregate["results"][0]["smoke"]["attempt"]["timed_out"],
            false
        );
        assert_eq!(aggregate["results"][1]["case_id"], "fixture");
        assert_eq!(
            aggregate["results"][1]["runner_error_code"], "accounting_destination_inside_scratch",
            "{route}: {aggregate}"
        );
        assert!(aggregate["results"][1]["smoke"].is_null());
        assert_eq!(aggregate["budget"]["observed_tokens"], 0);
        assert!(cli.smoke.rows().is_empty());
        assert!(
            !cli.smoke.platform.join("workflow-history.sqlite").exists(),
            "must not substitute a platform store"
        );
        cli.assert_cleaned();
        assert!(!scratch.exists());
    }
}
