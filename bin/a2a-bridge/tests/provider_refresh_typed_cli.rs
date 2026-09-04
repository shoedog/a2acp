#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

fn sha256(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    request: PathBuf,
    plan: PathBuf,
    evidence: PathBuf,
    receipt: PathBuf,
    production: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let candidate = root.join("candidate.bin");
        let production = root.join("production.bin");
        let rollback = root.join("rollback.bin");
        fs::write(&candidate, b"candidate-v2").unwrap();
        fs::write(&production, b"production-v1").unwrap();
        fs::write(&rollback, b"production-v1").unwrap();

        let request = root.join("request.json");
        write_json(
            &request,
            &json!({
                "schema_version": 2,
                "refresh_id": "refresh-20260904-v2",
                "components": [
                    {"provider":"codex","component":"codex-acp","version":"1.2.3","source":"https://registry.npmjs.org/codex-acp/-/codex-acp-1.2.3.tgz","size_bytes":5,"integrity":format!("sha256:{}", sha256(b"codex"))},
                    {"provider":"claude","component":"claude-agent-acp","version":"2.3.4","source":"https://registry.npmjs.org/claude-agent-acp/-/claude-agent-acp-2.3.4.tgz","size_bytes":6,"integrity":format!("sha256:{}", sha256(b"claude"))},
                    {"provider":"opencode","component":"opencode-ai","version":"3.4.5","source":"https://registry.npmjs.org/opencode-ai/-/opencode-ai-3.4.5.tgz","size_bytes":8,"integrity":format!("sha256:{}", sha256(b"opencode"))},
                    {"provider":"kiro","component":"kiro-cli","version":"4.5.6","source":"https://prod.download.cli.kiro.dev/stable/4.5.6/kiro.zip","size_bytes":4,"integrity":format!("sha256:{}", sha256(b"kiro"))}
                ],
                "targets": [
                    {"provider":"codex","mode":"acp","source_binding":"candidate","agent":"codex","selected_models":["gpt-5.6-sol"]},
                    {"provider":"claude","mode":"acp","source_binding":"candidate","agent":"claude","selected_models":["claude-opus-4-1"]},
                    {"provider":"kiro","mode":"acp","source_binding":"candidate","agent":"kiro","selected_models":["kiro-model"]},
                    {"provider":"opencode","mode":"deferred_catalog","source_binding":"candidate","selected_models":["opencode-go/alpha"]},
                    {"provider":"openrouter","mode":"deferred_catalog","source_binding":"candidate","default_model":"openrouter/free","selected_models":["vendor/free-tool"]}
                ],
                "opencode_subscription_models": ["opencode-go/alpha", "opencode-go/beta"],
                "openrouter_models": [
                    {"model":"vendor/free-tool","prompt_price":"0","completion_price":"0","supports_tools":true}
                ],
                "bindings": [
                    {"id":"candidate","role":"candidate","path":candidate,"sha256":sha256(b"candidate-v2")},
                    {"id":"production","role":"production","path":production,"sha256":sha256(b"production-v1")},
                    {"id":"rollback","role":"rollback","path":rollback,"sha256":sha256(b"production-v1")}
                ],
                "promotion_operations": [
                    {"type":"atomic_file_replace","id":"replace-config","candidate_binding":"candidate","production_binding":"production","rollback_binding":"rollback"},
                    {"type":"operator_restart_required","id":"restart-operator","service_id":"a2a-bridge"}
                ]
            }),
        );
        Self {
            plan: root.join("plan.json"),
            evidence: root.join("evidence.json"),
            receipt: root.join("receipt.json"),
            production,
            request,
            root,
            _dir: dir,
        }
    }

    fn command(&self, subcommand: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
        command.arg("provider-refresh").arg(subcommand);
        command
    }

    fn plan(&self) -> Output {
        self.command("plan")
            .arg("--request")
            .arg(&self.request)
            .arg("--out")
            .arg(&self.plan)
            .output()
            .unwrap()
    }

    fn check(&self) -> Output {
        self.command("check")
            .arg("--plan")
            .arg(&self.plan)
            .arg("--evidence")
            .arg(&self.evidence)
            .arg("--out")
            .arg(&self.receipt)
            .output()
            .unwrap()
    }

    fn request_value(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.request).unwrap()).unwrap()
    }

    fn set_request(&self, value: &Value) {
        write_json(&self.request, value);
    }

    fn plan_value(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.plan).unwrap()).unwrap()
    }

    fn create_green_evidence(&self) {
        let plan = self.plan_value();
        let mut checks = Vec::new();
        for required in plan["required_checks"].as_array().unwrap() {
            let id = required["id"].as_str().unwrap();
            let kind = required["kind"].as_str().unwrap();
            let provider = required["provider"].as_str().unwrap();
            let agent = required.get("agent").and_then(Value::as_str);
            let target = plan["targets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|target| target["provider"] == provider)
                .unwrap();
            let source_binding = target["source_binding"].as_str().unwrap();
            let source_sha256 = plan["bindings"]
                .as_array()
                .unwrap()
                .iter()
                .find(|binding| binding["id"] == source_binding)
                .unwrap()["sha256"]
                .as_str()
                .unwrap();
            let payload = match kind {
                "raw_acp_initialize" => json!({
                    "agent": agent.unwrap(),
                    "protocol_version": 1,
                    "initialized": true,
                    "session_created": false,
                    "prompt_calls": 0
                }),
                "doctor" => json!([{
                    "check": format!("provenance:{}:adapter", agent.unwrap()),
                    "status": "ok",
                    "detail": "fixture",
                    "remedy": ""
                }]),
                "models" => {
                    let selected = target["selected_models"].clone();
                    json!({
                        (agent.unwrap()): {
                            "available": true,
                            "models": selected,
                            "effort_levels": ["low"],
                            "modes": ["read-only"]
                        }
                    })
                }
                "opencode_catalog" => json!({
                    "provider": "opencode",
                    "prompt_calls": 0,
                    "models": [
                        {"id":"opencode-go/alpha","subscription_included":true},
                        {"id":"opencode-go/beta","subscription_included":true}
                    ]
                }),
                "openrouter_catalog" => json!({
                    "provider": "openrouter",
                    "prompt_calls": 0,
                    "models": [
                        {"id":"vendor/free-tool","prompt_price":"0","completion_price":"0","supports_tools":true}
                    ]
                }),
                other => panic!("unexpected check kind {other}"),
            };
            let artifact_value = json!({
                "schema_version": 1,
                "plan_id": plan["plan_id"],
                "provider": provider,
                "source_binding": source_binding,
                "source_sha256": source_sha256,
                "kind": kind,
                "agent": agent,
                "prompt_calls": 0,
                "session_created": false,
                "payload": payload
            });
            let artifact = self.root.join(format!("{}.json", id.replace('.', "-")));
            write_json(&artifact, &artifact_value);
            checks.push(json!({
                "id": id,
                "kind": kind,
                "artifact": artifact,
                "sha256": sha256(&fs::read(&artifact).unwrap())
            }));
        }
        write_json(
            &self.evidence,
            &json!({
                "schema_version": 2,
                "plan_id": plan["plan_id"],
                "checks": checks
            }),
        );
    }

    fn replace_evidence_artifact(&self, id: &str, value: Value) {
        let mut evidence: Value =
            serde_json::from_slice(&fs::read(&self.evidence).unwrap()).unwrap();
        let item = evidence["checks"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|item| item["id"] == id)
            .unwrap();
        let path = PathBuf::from(item["artifact"].as_str().unwrap());
        let mut envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope["payload"] = value;
        write_json(&path, &envelope);
        item["sha256"] = json!(sha256(&fs::read(&path).unwrap()));
        write_json(&self.evidence, &evidence);
    }

    fn replace_evidence_source_sha256(&self, id: &str, sha: &str) {
        let mut evidence: Value =
            serde_json::from_slice(&fs::read(&self.evidence).unwrap()).unwrap();
        let item = evidence["checks"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|item| item["id"] == id)
            .unwrap();
        let path = PathBuf::from(item["artifact"].as_str().unwrap());
        let mut envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope["source_sha256"] = json!(sha);
        write_json(&path, &envelope);
        item["sha256"] = json!(sha256(&fs::read(&path).unwrap()));
        write_json(&self.evidence, &evidence);
    }

    fn evidence_artifact(&self, id: &str) -> PathBuf {
        let evidence: Value = serde_json::from_slice(&fs::read(&self.evidence).unwrap()).unwrap();
        PathBuf::from(
            evidence["checks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == id)
                .unwrap()["artifact"]
                .as_str()
                .unwrap(),
        )
    }
}

#[test]
fn plan_and_check_are_provider_complete_and_non_promoting() {
    let fixture = Fixture::new();
    let production_before = fs::read(&fixture.production).unwrap();
    let planned = fixture.plan();
    assert!(planned.status.success(), "{planned:?}");
    assert_eq!(fs::read(&fixture.production).unwrap(), production_before);
    let plan = fixture.plan_value();
    assert_eq!(plan["schema_version"], 2);
    assert_eq!(plan["authority"], "resolution_and_verification_plan_only");
    assert_eq!(plan["promotion_ready"], false);
    assert_eq!(plan["required_checks"].as_array().unwrap().len(), 11);
    assert_eq!(plan["plan_id"].as_str().unwrap().len(), 64);
    assert!(!serde_json::to_string(&plan).unwrap().contains("executable"));
    assert_eq!(
        fs::metadata(&fixture.plan).unwrap().permissions().mode() & 0o777,
        0o600
    );

    fixture.create_green_evidence();
    let checked = fixture.check();
    assert!(checked.status.success(), "{checked:?}");
    assert_eq!(fs::read(&fixture.production).unwrap(), production_before);
    let receipt: Value = serde_json::from_slice(&fs::read(&fixture.receipt).unwrap()).unwrap();
    assert_eq!(receipt["authority"], "provider_free_verification_only");
    assert_eq!(receipt["status"], "pass");
    assert_eq!(receipt["promotion_ready"], false);
    assert_eq!(receipt["checks"].as_array().unwrap().len(), 11);
}

#[test]
fn plan_rejects_missing_provider_and_generic_command_escape_fields() {
    let missing = Fixture::new();
    let mut request = missing.request_value();
    request["targets"]
        .as_array_mut()
        .unwrap()
        .retain(|target| target["provider"] != "openrouter");
    missing.set_request(&request);
    let rejected = missing.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("exactly one target for each provider"));

    let command = Fixture::new();
    let mut request = command.request_value();
    request["promotion_operations"][0]["executable"] = json!("/bin/sh");
    request["promotion_operations"][0]["argv"] = json!(["-c", "anything"]);
    request["promotion_operations"][0]["shell"] = json!("anything");
    request["promotion_operations"][0]["env"] = json!({"TOKEN":"hidden"});
    request["required_checks"] = json!([]);
    let injected = command.root.join("injected-request.json");
    write_json(&injected, &request);
    let output = command.root.join("injected-plan.json");
    let rejected = command
        .command("plan")
        .arg("--request")
        .arg(injected)
        .arg("--out")
        .arg(output)
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("unknown field"));
}

#[test]
fn plan_enforces_openrouter_free_tools_and_opencode_subscription_selection() {
    let paid = Fixture::new();
    let mut request = paid.request_value();
    request["openrouter_models"][0]["prompt_price"] = json!("0.0001");
    paid.set_request(&request);
    assert!(paid.plan().status.success());
    paid.create_green_evidence();
    let rejected = paid.check();
    assert!(!rejected.status.success());
    assert!(
        stderr(&rejected).contains("OpenRouter catalog evidence does not match the free-only plan")
    );

    let unentitled = Fixture::new();
    let mut request = unentitled.request_value();
    request["targets"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|target| target["provider"] == "opencode")
        .unwrap()["selected_models"] = json!(["opencode-go/not-in-plan"]);
    unentitled.set_request(&request);
    let rejected = unentitled.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("operator-asserted OpenCode subscription set"));

    let empty = Fixture::new();
    let mut request = empty.request_value();
    request["targets"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|target| target["provider"] == "opencode")
        .unwrap()["selected_models"] = json!([]);
    empty.set_request(&request);
    let rejected = empty.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("opencode selected models must be a non-empty bounded set"));
}

#[test]
fn check_requires_the_exact_derived_provider_complete_set() {
    let fixture = Fixture::new();
    assert!(fixture.plan().status.success());
    fixture.create_green_evidence();
    let mut evidence: Value =
        serde_json::from_slice(&fs::read(&fixture.evidence).unwrap()).unwrap();
    evidence["checks"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item["id"].as_str().unwrap().starts_with("codex."));
    write_json(&fixture.evidence, &evidence);
    let rejected = fixture.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("exact derived provider check set"));
}

#[test]
fn check_revalidates_openrouter_and_opencode_catalog_semantics() {
    let paid = Fixture::new();
    assert!(paid.plan().status.success());
    paid.create_green_evidence();
    paid.replace_evidence_artifact(
        "openrouter.openrouter_catalog",
        json!({
            "provider":"openrouter",
            "prompt_calls":0,
            "models":[{"id":"vendor/free-tool","prompt_price":"0.0001","completion_price":"0","supports_tools":true}]
        }),
    );
    let rejected = paid.check();
    assert!(!rejected.status.success());
    assert!(
        stderr(&rejected).contains("OpenRouter catalog evidence does not match the free-only plan")
    );

    let missing = Fixture::new();
    assert!(missing.plan().status.success());
    missing.create_green_evidence();
    missing.replace_evidence_artifact(
        "opencode.opencode_catalog",
        json!({
            "provider":"opencode",
            "prompt_calls":0,
            "models":[{"id":"opencode-go/beta","subscription_included":true}]
        }),
    );
    let rejected = missing.check();
    assert!(!rejected.status.success());
    assert!(
        stderr(&rejected).contains("OpenCode catalog evidence omits a selected subscription model")
    );
}

#[test]
fn check_rejects_prompted_initialize_and_missing_selected_model() {
    let prompted = Fixture::new();
    assert!(prompted.plan().status.success());
    prompted.create_green_evidence();
    prompted.replace_evidence_artifact(
        "codex.raw_acp_initialize",
        json!({
            "agent":"codex",
            "protocol_version":1,
            "initialized":true,
            "session_created":true,
            "prompt_calls":1
        }),
    );
    let rejected = prompted.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("initialize-only protocol v1"));

    let missing_model = Fixture::new();
    assert!(missing_model.plan().status.success());
    missing_model.create_green_evidence();
    missing_model.replace_evidence_artifact(
        "claude.models",
        json!({"claude":{"available":true,"models":["different-model"]}}),
    );
    let rejected = missing_model.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("models evidence omits a selected model"));

    let stale = Fixture::new();
    assert!(stale.plan().status.success());
    stale.create_green_evidence();
    stale.replace_evidence_source_sha256("kiro.doctor", &"a".repeat(64));
    let rejected = stale.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("exact plan, candidate, probe, and zero-prompt state"));
}

#[test]
fn check_rejects_bad_doctor_empty_models_artifact_drift_and_plan_drift() {
    let doctor = Fixture::new();
    assert!(doctor.plan().status.success());
    doctor.create_green_evidence();
    doctor.replace_evidence_artifact(
        "codex.doctor",
        json!([{"check":"provenance:codex:adapter","status":"fail","detail":"bad","remedy":"fix"}]),
    );
    let rejected = doctor.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("doctor evidence contains a failing check"));

    let models = Fixture::new();
    assert!(models.plan().status.success());
    models.create_green_evidence();
    models.replace_evidence_artifact(
        "kiro.models",
        json!({"kiro":{"available":true,"models":[]}}),
    );
    let rejected = models.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("models evidence is unavailable or empty"));

    let artifact = Fixture::new();
    assert!(artifact.plan().status.success());
    artifact.create_green_evidence();
    fs::write(artifact.evidence_artifact("claude.doctor"), b"changed").unwrap();
    let rejected = artifact.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("binding drift"));

    let plan = Fixture::new();
    assert!(plan.plan().status.success());
    plan.create_green_evidence();
    let mut value = plan.plan_value();
    value["promotion_ready"] = json!(true);
    write_json(&plan.plan, &value);
    let rejected = plan.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("plan identity or authority mismatch"));
}

#[test]
fn file_boundaries_reject_existing_output_symlink_input_and_public_parent() {
    let existing = Fixture::new();
    fs::write(&existing.plan, b"do-not-replace").unwrap();
    let rejected = existing.plan();
    assert!(!rejected.status.success());
    assert_eq!(fs::read(&existing.plan).unwrap(), b"do-not-replace");

    let linked = Fixture::new();
    let symlink_request = linked.root.join("request-link.json");
    symlink(&linked.request, &symlink_request).unwrap();
    let output = linked.root.join("linked-plan.json");
    let rejected = linked
        .command("plan")
        .arg("--request")
        .arg(symlink_request)
        .arg("--out")
        .arg(output)
        .output()
        .unwrap();
    assert!(!rejected.status.success());

    let public = Fixture::new();
    let public_parent = public.root.join("public");
    fs::create_dir(&public_parent).unwrap();
    fs::set_permissions(&public_parent, fs::Permissions::from_mode(0o755)).unwrap();
    let rejected = public
        .command("plan")
        .arg("--request")
        .arg(&public.request)
        .arg("--out")
        .arg(public_parent.join("plan.json"))
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("parent must be owner-private"));
}

#[test]
fn plan_accepts_the_closed_typed_operation_vocabulary() {
    let fixture = Fixture::new();
    let mut request = fixture.request_value();
    request["promotion_operations"] = json!([
        {"type":"atomic_file_replace","id":"replace-config","candidate_binding":"candidate","production_binding":"production","rollback_binding":"rollback"},
        {"type":"operator_restart_required","id":"restart-operator","service_id":"a2a-bridge"}
    ]);
    fixture.set_request(&request);
    let planned = fixture.plan();
    assert!(planned.status.success(), "{planned:?}");
    assert_eq!(
        fixture.plan_value()["promotion_operations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let unsupported = Fixture::new();
    let mut request = unsupported.request_value();
    request["promotion_operations"] = json!([
        {"type":"image_tag_move","id":"move-image","runtime":"docker"},
        {"type":"operator_restart_required","id":"restart-operator","service_id":"a2a-bridge"}
    ]);
    unsupported.set_request(&request);
    let rejected = unsupported.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("unknown variant"));
}

#[test]
fn plan_rejects_role_aliases_unknown_sources_and_missing_restart_marker() {
    let aliased = Fixture::new();
    let mut request = aliased.request_value();
    let production = request["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|binding| binding["id"] == "production")
        .unwrap()
        .clone();
    let candidate = request["bindings"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|binding| binding["id"] == "candidate")
        .unwrap();
    candidate["path"] = production["path"].clone();
    candidate["sha256"] = production["sha256"].clone();
    aliased.set_request(&request);
    let rejected = aliased.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("binding paths must be unique"));

    let source = Fixture::new();
    let mut request = source.request_value();
    request["targets"][0]["source_binding"] = json!("missing-candidate-manifest");
    source.set_request(&request);
    let rejected = source.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("every provider target must bind an exact candidate"));

    let restart = Fixture::new();
    let mut request = restart.request_value();
    request["promotion_operations"]
        .as_array_mut()
        .unwrap()
        .retain(|operation| operation["type"] != "operator_restart_required");
    restart.set_request(&request);
    let rejected = restart.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("exactly one operator restart-required marker"));
}

#[test]
fn semantic_plan_id_ignores_raw_whitespace_and_unordered_set_order() {
    let fixture = Fixture::new();
    assert!(fixture.plan().status.success());
    let first = fixture.plan_value();
    let mut reordered = fixture.request_value();
    reordered["components"].as_array_mut().unwrap().reverse();
    reordered["targets"].as_array_mut().unwrap().reverse();
    reordered["opencode_subscription_models"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let second_request = fixture.root.join("reordered-request.json");
    fs::write(&second_request, serde_json::to_vec(&reordered).unwrap()).unwrap();
    let second_plan = fixture.root.join("reordered-plan.json");
    let planned = fixture
        .command("plan")
        .arg("--request")
        .arg(second_request)
        .arg("--out")
        .arg(&second_plan)
        .output()
        .unwrap();
    assert!(planned.status.success(), "{planned:?}");
    let second: Value = serde_json::from_slice(&fs::read(second_plan).unwrap()).unwrap();
    assert_eq!(first["plan_id"], second["plan_id"]);
    assert_ne!(first["request_sha256"], second["request_sha256"]);
}

#[test]
fn help_exposes_plan_check_and_the_separate_future_promotion_boundary() {
    let help = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["provider-refresh", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success(), "{help:?}");
    let stdout = String::from_utf8(help.stdout).unwrap();
    assert!(stdout.contains("provider-refresh plan"));
    assert!(stdout.contains("provider-refresh check"));
    assert!(stdout.contains("does not implement `promote`"));
    assert!(stdout.contains("does not authorize a billable turn"));

    let promote = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["provider-refresh", "promote"])
        .output()
        .unwrap();
    assert!(!promote.status.success());
    assert!(stderr(&promote).contains("promote is not implemented in typed slice A"));
}
