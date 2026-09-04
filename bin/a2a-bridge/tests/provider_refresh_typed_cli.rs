#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use base64::Engine as _;
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

fn sri(seed: u8) -> String {
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode([seed; 64])
    )
}

fn npm_component(kind: &str, package: &str, version: &str, seed: u8) -> Value {
    let basename = package.rsplit('/').next().unwrap();
    json!({
        "kind": kind,
        "version": version,
        "source": {
            "type": "npm",
            "package": package,
            "tarball_url": format!("https://registry.npmjs.org/{package}/-/{basename}-{version}.tgz"),
            "size_bytes": u64::from(seed) + 100,
            "integrity": sri(seed)
        }
    })
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
        let promotion_payload = root.join("candidate-config.bin");
        let production = root.join("production.bin");
        let rollback = root.join("rollback.bin");
        fs::write(&promotion_payload, b"candidate-v2").unwrap();
        fs::write(&production, b"production-v1").unwrap();
        fs::write(&rollback, b"production-v1").unwrap();

        let codex_executable = root.join("codex-acp");
        let claude_executable = root.join("claude-agent-acp");
        let kiro_executable = root.join("kiro-cli");
        let codex_standalone = root.join("codex");
        let claude_standalone = root.join("claude");
        for (path, bytes) in [
            (&codex_executable, b"codex-acp".as_slice()),
            (&claude_executable, b"claude-acp".as_slice()),
            (&kiro_executable, b"kiro-cli".as_slice()),
            (&codex_standalone, b"codex-standalone".as_slice()),
            (&claude_standalone, b"claude-standalone".as_slice()),
        ] {
            fs::write(path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }

        let codex_components = vec![
            npm_component(
                "codex_acp_adapter",
                "@agentclientprotocol/codex-acp",
                "1.2.3",
                1,
            ),
            npm_component("codex_nested_cli", "@openai/codex", "0.3.4", 2),
            json!({
                "kind":"codex_standalone_cli",
                "version":"0.3.5",
                "source":{
                    "type":"managed_executable","manager":"mise","package":"@openai/codex",
                    "path":codex_standalone,"size_bytes":fs::metadata(&codex_standalone).unwrap().len(),
                    "sha256":sha256(&fs::read(&codex_standalone).unwrap())
                }
            }),
        ];
        let claude_sdk_manifest = root.join("claude-sdk-package.json");
        fs::write(&claude_sdk_manifest, b"claude-sdk-manifest").unwrap();
        let claude_components = vec![
            npm_component(
                "claude_acp_adapter",
                "@agentclientprotocol/claude-agent-acp",
                "2.3.4",
                3,
            ),
            npm_component(
                "claude_agent_sdk",
                "@anthropic-ai/claude-agent-sdk",
                "0.3.5",
                4,
            ),
            json!({
                "kind":"claude_bundled_cli","version":"2.3.5",
                "source":{
                    "type":"bundled_cli","parent":"claude_agent_sdk","parent_version":"0.3.5",
                    "manifest_sha256":sha256(&fs::read(&claude_sdk_manifest).unwrap())
                }
            }),
            json!({
                "kind":"claude_standalone_cli","version":"2.3.6",
                "source":{
                    "type":"managed_executable","manager":"native_updater","package":"claude",
                    "path":claude_standalone,"size_bytes":fs::metadata(&claude_standalone).unwrap().len(),
                    "sha256":sha256(&fs::read(&claude_standalone).unwrap())
                }
            }),
        ];
        let opencode_components = vec![npm_component("opencode_cli", "opencode-ai", "3.4.5", 5)];
        let kiro_components = vec![json!({
            "kind":"kiro_cli","version":"4.5.6",
            "source":{
                "type":"kiro_stable_archive","architecture":"aarch64_apple_darwin",
                "url":"https://prod.download.cli.kiro.dev/stable/4.5.6/kirocli-aarch64-apple-darwin.zip",
                "size_bytes":456,"sha256":sha256(b"kiro-archive")
            }
        })];
        let components: Vec<_> = codex_components
            .iter()
            .chain(&claude_components)
            .chain(&opencode_components)
            .chain(&kiro_components)
            .cloned()
            .collect();

        let write_artifact = |name: &str, bytes: &[u8]| {
            let path = root.join(name);
            fs::write(&path, bytes).unwrap();
            path
        };
        let codex_tree = write_artifact("codex-tree.json", b"codex tree");
        let codex_config = write_artifact("codex-config.toml", b"codex config");
        let claude_tree = write_artifact("claude-tree.json", b"claude tree");
        let claude_config = write_artifact("claude-config.toml", b"claude config");
        let kiro_config = write_artifact("kiro-config.toml", b"kiro config");
        let opencode_catalog = root.join("opencode-catalog.json");
        write_json(
            &opencode_catalog,
            &json!({
                "provider":"opencode","prompt_calls":0,
                "models":[
                    {"id":"opencode-go/alpha","subscription_included":true},
                    {"id":"opencode-go/beta","subscription_included":true}
                ]
            }),
        );
        let openrouter_catalog = root.join("openrouter-catalog.json");
        write_json(
            &openrouter_catalog,
            &json!({
                "provider":"openrouter","prompt_calls":0,
                "models":[
                    {"id":"vendor/free-tool","prompt_price":"0","completion_price":"0","supports_tools":true}
                ]
            }),
        );
        let artifact = |id: &str, kind: &str, path: &Path| {
            json!({
                "id":id,"kind":kind,"path":path,
                "size_bytes":fs::metadata(path).unwrap().len(),
                "sha256":sha256(&fs::read(path).unwrap())
            })
        };

        let manifests = [
            (
                "codex-source",
                json!({
                    "schema_version":1,"kind":"candidate_manifest","provider":"codex",
                    "components":codex_components,
                    "artifacts":[
                        artifact("codex-executable","executable",&codex_executable),
                        artifact("codex-tree","package_tree_manifest",&codex_tree),
                        artifact("codex-config","config",&codex_config)
                    ],
                    "execution":{"type":"host","executable_artifact":"codex-executable"},
                    "promotion_payload_bindings":["candidate-config"]
                }),
                "candidate_manifest",
            ),
            (
                "claude-source",
                json!({
                    "schema_version":1,"kind":"candidate_manifest","provider":"claude",
                    "components":claude_components,
                    "artifacts":[
                        artifact("claude-executable","executable",&claude_executable),
                        artifact("claude-tree","package_tree_manifest",&claude_tree),
                        artifact("claude-config","config",&claude_config)
                    ],
                    "execution":{"type":"host","executable_artifact":"claude-executable"}
                }),
                "candidate_manifest",
            ),
            (
                "kiro-source",
                json!({
                    "schema_version":1,"kind":"candidate_manifest","provider":"kiro",
                    "components":kiro_components,
                    "artifacts":[
                        artifact("kiro-executable","executable",&kiro_executable),
                        artifact("kiro-config","config",&kiro_config)
                    ],
                    "execution":{"type":"host","executable_artifact":"kiro-executable"}
                }),
                "candidate_manifest",
            ),
            (
                "opencode-source",
                json!({
                    "schema_version":1,"kind":"catalog_resolution","provider":"opencode",
                    "components":opencode_components,
                    "artifacts":[artifact("opencode-catalog","catalog_snapshot",&opencode_catalog)]
                }),
                "catalog_resolution",
            ),
            (
                "openrouter-source",
                json!({
                    "schema_version":1,"kind":"catalog_resolution","provider":"openrouter",
                    "components":[],
                    "artifacts":[artifact("openrouter-catalog","catalog_snapshot",&openrouter_catalog)]
                }),
                "catalog_resolution",
            ),
        ];
        let mut source_bindings = Vec::new();
        for (id, manifest, role) in manifests {
            let path = root.join(format!("{id}.json"));
            write_json(&path, &manifest);
            source_bindings.push(json!({
                "id":id,"role":role,"path":path,"sha256":sha256(&fs::read(&path).unwrap())
            }));
        }

        let request = root.join("request.json");
        let mut bindings = source_bindings;
        bindings.extend([
            json!({"id":"candidate-config","role":"promotion_payload","path":promotion_payload,"sha256":sha256(b"candidate-v2")}),
            json!({"id":"production","role":"production","path":production,"sha256":sha256(b"production-v1")}),
            json!({"id":"rollback","role":"rollback","path":rollback,"sha256":sha256(b"production-v1")}),
        ]);
        write_json(
            &request,
            &json!({
                "schema_version": 2,
                "refresh_id": "refresh-20260904-v2",
                "components": components,
                "targets": [
                    {"provider":"codex","mode":"acp","source_binding":"codex-source","agent":"codex","selected_models":["gpt-5.6-sol"]},
                    {"provider":"claude","mode":"acp","source_binding":"claude-source","agent":"claude","selected_models":["claude-opus-4-1"]},
                    {"provider":"kiro","mode":"acp","source_binding":"kiro-source","agent":"kiro","selected_models":["kiro-model"]},
                    {"provider":"opencode","mode":"deferred_catalog","source_binding":"opencode-source","selected_models":["opencode-go/alpha"]},
                    {"provider":"openrouter","mode":"deferred_catalog","source_binding":"openrouter-source","default_model":"openrouter/free","selected_models":["vendor/free-tool"]}
                ],
                "opencode_subscription_models": ["opencode-go/alpha", "opencode-go/beta"],
                "openrouter_models": [
                    {"model":"vendor/free-tool","prompt_price":"0","completion_price":"0","supports_tools":true}
                ],
                "bindings": bindings,
                "promotion_operations": [
                    {"type":"atomic_file_replace","id":"replace-config","owner_source_binding":"codex-source","candidate_binding":"candidate-config","production_binding":"production","rollback_binding":"rollback"},
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

    fn mutate_source_manifest(&self, binding_id: &str, mutate: impl FnOnce(&mut Value)) {
        let mut request = self.request_value();
        let binding = request["bindings"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|binding| binding["id"] == binding_id)
            .unwrap();
        let path = PathBuf::from(binding["path"].as_str().unwrap());
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        mutate(&mut manifest);
        write_json(&path, &manifest);
        binding["sha256"] = json!(sha256(&fs::read(&path).unwrap()));
        self.set_request(&request);
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
            let source_path = PathBuf::from(
                plan["bindings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|binding| binding["id"] == source_binding)
                    .unwrap()["path"]
                    .as_str()
                    .unwrap(),
            );
            let source_manifest: Value =
                serde_json::from_slice(&fs::read(source_path).unwrap()).unwrap();
            let version = |component_kind: &str| {
                plan["components"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|component| component["kind"] == component_kind)
                    .unwrap()["version"]
                    .as_str()
                    .unwrap()
            };
            let payload = match kind {
                "raw_acp_initialize" => {
                    let component_kind = match provider {
                        "codex" => "codex_acp_adapter",
                        "claude" => "claude_acp_adapter",
                        "kiro" => "kiro_cli",
                        other => panic!("unexpected ACP provider {other}"),
                    };
                    json!({
                        "agent": agent.unwrap(),
                        "protocol_version": 1,
                        "initialized": true,
                        "session_created": false,
                        "prompt_calls": 0,
                        "agent_info": {"version":version(component_kind)}
                    })
                }
                "doctor" => {
                    let mut rows = match source_manifest["execution"]["type"].as_str().unwrap() {
                        "host" => {
                            let execution_artifact = source_manifest["execution"]
                                ["executable_artifact"]
                                .as_str()
                                .unwrap();
                            let executable = PathBuf::from(
                                source_manifest["artifacts"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .find(|artifact| artifact["id"] == execution_artifact)
                                    .unwrap()["path"]
                                    .as_str()
                                    .unwrap(),
                            );
                            vec![json!({
                                "check":format!("provenance:{}:execution",agent.unwrap()),
                                "status":"ok",
                                "detail":format!("kind=acp execution=host configured_cmd={} executable={:?}",agent.unwrap(),executable),
                                "remedy":""
                            })]
                        }
                        "container" => vec![
                            json!({
                                "check":format!("provenance:{}:execution",agent.unwrap()),
                                "status":"ok","detail":"kind=acp execution=container runtime=docker","remedy":""
                            }),
                            json!({
                                "check":format!("provenance:{}:image",agent.unwrap()),
                                "status":"ok",
                                "detail":format!("runtime=docker immutable_id={}",source_manifest["execution"]["immutable_id"].as_str().unwrap()),
                                "remedy":""
                            }),
                        ],
                        other => panic!("unexpected execution type {other}"),
                    };
                    match provider {
                        "codex" => {
                            rows.push(json!({
                                "check":format!("provenance:{}:adapter",agent.unwrap()),"status":"ok",
                                "detail":format!("package=@agentclientprotocol/codex-acp version={}",version("codex_acp_adapter")),"remedy":""
                            }));
                            rows.push(json!({
                                "check":format!("provenance:{}:agent-cli",agent.unwrap()),"status":"ok",
                                "detail":format!("package=@openai/codex version={}",version("codex_nested_cli")),"remedy":""
                            }));
                        }
                        "claude" => {
                            rows.push(json!({
                                "check":format!("provenance:{}:adapter",agent.unwrap()),"status":"ok",
                                "detail":format!("package=@agentclientprotocol/claude-agent-acp version={}",version("claude_acp_adapter")),"remedy":""
                            }));
                            rows.push(json!({
                                "check":format!("provenance:{}:agent-cli",agent.unwrap()),"status":"ok",
                                "detail":format!("package=@anthropic-ai/claude-agent-sdk version={} bundled_cli_version={}",version("claude_agent_sdk"),version("claude_bundled_cli")),"remedy":""
                            }));
                        }
                        "kiro" => {}
                        other => panic!("unexpected doctor provider {other}"),
                    }
                    json!(rows)
                }
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

    fn mutate_evidence_payload(&self, id: &str, mutate: impl FnOnce(&mut Value)) {
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
        mutate(&mut envelope["payload"]);
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
fn plan_and_check_cover_the_closed_graph_and_report_deferred_components() {
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
    assert_eq!(plan["components"].as_array().unwrap().len(), 9);
    assert_eq!(plan["deferred_components"].as_array().unwrap().len(), 3);
    assert!(!serde_json::to_string(&plan).unwrap().contains("\"argv\""));
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
    assert_eq!(receipt["status"], "pass_with_deferred_components");
    assert_eq!(receipt["promotion_ready"], false);
    assert_eq!(receipt["checks"].as_array().unwrap().len(), 11);
    assert_eq!(receipt["deferred_components"], plan["deferred_components"]);
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
    let detached = Fixture::new();
    assert!(detached.plan().status.success());
    detached.create_green_evidence();
    detached.replace_evidence_artifact(
        "openrouter.openrouter_catalog",
        json!({
            "provider":"openrouter",
            "prompt_calls":0,
            "models":[{"id":"vendor/free-tool","prompt_price":"0.0001","completion_price":"0","supports_tools":true}]
        }),
    );
    let rejected = detached.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("does not match the bound resolution snapshot"));

    let missing = Fixture::new();
    let missing_payload = json!({
        "provider":"opencode",
        "prompt_calls":0,
        "models":[{"id":"opencode-go/beta","subscription_included":true}]
    });
    missing.mutate_source_manifest("opencode-source", |manifest| {
        let snapshot = &mut manifest["artifacts"][0];
        let path = PathBuf::from(snapshot["path"].as_str().unwrap());
        write_json(&path, &missing_payload);
        snapshot["size_bytes"] = json!(fs::metadata(&path).unwrap().len());
        snapshot["sha256"] = json!(sha256(&fs::read(&path).unwrap()));
    });
    assert!(missing.plan().status.success());
    missing.create_green_evidence();
    missing.replace_evidence_artifact("opencode.opencode_catalog", missing_payload);
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
        {"type":"atomic_file_replace","id":"replace-config","owner_source_binding":"codex-source","candidate_binding":"candidate-config","production_binding":"production","rollback_binding":"rollback"},
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
        .find(|binding| binding["id"] == "candidate-config")
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
    assert!(stderr(&rejected).contains("typed manifest role"));

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
fn plan_rejects_incomplete_components_and_noncanonical_resolution_sources() {
    let incomplete = Fixture::new();
    let mut request = incomplete.request_value();
    request["components"]
        .as_array_mut()
        .unwrap()
        .retain(|component| component["kind"] != "codex_nested_cli");
    incomplete.set_request(&request);
    let rejected = incomplete.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("closed component graph"));

    let npm = Fixture::new();
    let mut request = npm.request_value();
    request["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|component| component["kind"] == "codex_acp_adapter")
        .unwrap()["source"]["tarball_url"] = json!("https://registry.npmjs.org/wrong.tgz");
    npm.set_request(&request);
    let rejected = npm.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("canonical package tarball"));

    let kiro = Fixture::new();
    let mut request = kiro.request_value();
    request["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|component| component["kind"] == "kiro_cli")
        .unwrap()["source"]["url"] =
        json!("https://prod.download.cli.kiro.dev/stable/latest/kiro.zip");
    kiro.set_request(&request);
    let rejected = kiro.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("architecture-specific stable archive"));
}

#[test]
fn plan_rejects_opaque_wrong_or_reused_source_manifests() {
    let opaque = Fixture::new();
    let mut request = opaque.request_value();
    let binding = request["bindings"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|binding| binding["id"] == "codex-source")
        .unwrap();
    let path = PathBuf::from(binding["path"].as_str().unwrap());
    fs::write(&path, b"candidate-v2").unwrap();
    binding["sha256"] = json!(sha256(b"candidate-v2"));
    opaque.set_request(&request);
    let rejected = opaque.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("invalid typed source manifest JSON"));

    let wrong = Fixture::new();
    wrong.mutate_source_manifest("codex-source", |manifest| {
        manifest["provider"] = json!("claude");
    });
    let rejected = wrong.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("provider mismatch"));

    let reused = Fixture::new();
    let mut request = reused.request_value();
    request["targets"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|target| target["provider"] == "claude")
        .unwrap()["source_binding"] = json!("codex-source");
    reused.set_request(&request);
    let rejected = reused.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("distinct source bindings"));
}

#[test]
fn plan_requires_manifest_owned_promotion_payload() {
    let fixture = Fixture::new();
    fixture.mutate_source_manifest("codex-source", |manifest| {
        manifest["promotion_payload_bindings"] = json!([]);
    });
    let rejected = fixture.plan();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("payload must be owned"));
}

#[test]
fn check_binds_raw_adapter_version_doctor_packages_and_host_executable() {
    let camel_case = Fixture::new();
    assert!(camel_case.plan().status.success());
    camel_case.create_green_evidence();
    camel_case.mutate_evidence_payload("kiro.raw_acp_initialize", |payload| {
        let agent_info = payload
            .as_object_mut()
            .unwrap()
            .remove("agent_info")
            .unwrap();
        payload["agentInfo"] = agent_info;
    });
    assert!(camel_case.check().status.success());

    let raw = Fixture::new();
    assert!(raw.plan().status.success());
    raw.create_green_evidence();
    raw.mutate_evidence_payload("codex.raw_acp_initialize", |payload| {
        payload["agent_info"]["version"] = json!("9.9.9");
    });
    let rejected = raw.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("exact candidate version"));

    let package = Fixture::new();
    assert!(package.plan().status.success());
    package.create_green_evidence();
    package.mutate_evidence_payload("claude.doctor", |payload| {
        payload
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["check"] == "provenance:claude:agent-cli")
            .unwrap()["detail"] =
            json!("package=@anthropic-ai/claude-agent-sdk version=9.9.9 bundled_cli_version=2.3.5");
    });
    let rejected = package.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("Claude candidate"));

    let executable = Fixture::new();
    assert!(executable.plan().status.success());
    executable.create_green_evidence();
    executable.mutate_evidence_payload("kiro.doctor", |payload| {
        payload[0]["detail"] =
            json!("kind=acp execution=host configured_cmd=kiro executable=\"/wrong\"");
    });
    let rejected = executable.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("exact host executable"));
}

#[test]
fn container_identity_is_checked_against_doctor_image_provenance() {
    let fixture = Fixture::new();
    let image_receipt = fixture.root.join("codex-image.json");
    fs::write(&image_receipt, b"immutable image receipt").unwrap();
    fixture.mutate_source_manifest("codex-source", |manifest| {
        manifest["artifacts"].as_array_mut().unwrap().push(json!({
            "id":"codex-image","kind":"image_receipt","path":image_receipt,
            "size_bytes":fs::metadata(&image_receipt).unwrap().len(),
            "sha256":sha256(&fs::read(&image_receipt).unwrap())
        }));
        manifest["execution"] = json!({
            "type":"container","image_artifact":"codex-image",
            "immutable_id":format!("sha256:{}","a".repeat(64))
        });
    });
    assert!(fixture.plan().status.success());
    fixture.create_green_evidence();
    assert!(fixture.check().status.success());

    let drift = Fixture::new();
    let image_receipt = drift.root.join("codex-image.json");
    fs::write(&image_receipt, b"immutable image receipt").unwrap();
    drift.mutate_source_manifest("codex-source", |manifest| {
        manifest["artifacts"].as_array_mut().unwrap().push(json!({
            "id":"codex-image","kind":"image_receipt","path":image_receipt,
            "size_bytes":fs::metadata(&image_receipt).unwrap().len(),
            "sha256":sha256(&fs::read(&image_receipt).unwrap())
        }));
        manifest["execution"] = json!({
            "type":"container","image_artifact":"codex-image",
            "immutable_id":format!("sha256:{}","a".repeat(64))
        });
    });
    assert!(drift.plan().status.success());
    drift.create_green_evidence();
    drift.mutate_evidence_payload("codex.doctor", |payload| {
        payload
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["check"] == "provenance:codex:image")
            .unwrap()["detail"] = json!(format!(
            "runtime=docker immutable_id=sha256:{}",
            "b".repeat(64)
        ));
    });
    let rejected = drift.check();
    assert!(!rejected.status.success());
    assert!(stderr(&rejected).contains("immutable candidate image"));
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
    let top = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .arg("help")
        .output()
        .unwrap();
    assert!(top.status.success(), "{top:?}");
    let top_stdout = String::from_utf8(top.stdout).unwrap();
    assert!(top_stdout.contains("provider-refresh"));
    assert!(top_stdout.contains("promotion, restart, resolution, and billable turns are separate"));

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
