use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const OPENCODE_VOLUME: &str = "/Users/wesleyjinks/.config/a2a-creds/opencode/auth.json:/root/.local/share/opencode/auth.json:ro";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn container_config() -> String {
    fs::read_to_string(repo_root().join("examples/a2a-bridge.containerized.toml")).unwrap()
}

fn config_paths() -> [PathBuf; 2] {
    let root = repo_root();
    [
        root.join("examples/a2a-bridge.containerized.toml"),
        root.join("examples/a2a-bridge.containerized.podman.toml"),
    ]
}

fn table<'a>(value: &'a toml::Value, name: &str) -> &'a toml::value::Table {
    value
        .get(name)
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("missing table {name}"))
}

fn array<'a>(value: &'a toml::Value, name: &str) -> &'a Vec<toml::Value> {
    value
        .get(name)
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("missing array {name}"))
}

fn text<'a>(value: &'a toml::Value, name: &str) -> &'a str {
    value
        .get(name)
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("missing string {name}"))
}

fn workflow<'a>(config: &'a toml::Value, id: &str) -> &'a toml::Value {
    array(config, "workflows")
        .iter()
        .find(|workflow| text(workflow, "id") == id)
        .unwrap_or_else(|| panic!("missing workflow {id}"))
}

fn node<'a>(workflow: &'a toml::Value, id: &str) -> &'a toml::Value {
    array(workflow, "nodes")
        .iter()
        .find(|node| text(node, "id") == id)
        .unwrap_or_else(|| panic!("missing {id} node"))
}

fn node_inputs(node: &toml::Value) -> Vec<&str> {
    array(node, "inputs")
        .iter()
        .map(|value| value.as_str().expect("workflow input must be a string"))
        .collect()
}

fn assert_synth_renders_dependencies(path: &Path, workflow_id: &str) -> Result<(), String> {
    let raw = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let config = raw
        .parse::<toml::Value>()
        .map_err(|error| error.to_string())?;
    let synth = node(workflow(&config, workflow_id), "synth");
    let file = text(synth, "prompt_file");
    let template =
        fs::read_to_string(path.parent().unwrap().join(file)).map_err(|error| error.to_string())?;
    let mut rendered = template.clone();
    for input in node_inputs(synth) {
        let marker = format!(
            "[{workflow_id}/{input}: {}]",
            if input == "opencode" {
                "node failed"
            } else {
                "report"
            }
        );
        let token = format!("{{{{{input}}}}}");
        if !template.contains(&token) {
            return Err(format!("{workflow_id} synthesis omits {token}"));
        }
        rendered = rendered.replace(&token, &marker);
        if !rendered.contains(&marker) {
            return Err(format!("{workflow_id} did not render {input}"));
        }
    }
    Ok(())
}

fn filter_allows(filter: &Path, host: &str) -> bool {
    let mut child = Command::new("grep")
        .args(["-Eq", "-f"])
        .arg(filter)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.as_mut().unwrap(), "{host}").unwrap();
    child.wait().unwrap().success()
}

// OpenCode v1.18.27 reads auth.json as provider-keyed records. The Go provider needs `opencode-go`, while
// `opencode` preserves the concurrent OpenCode Zen credential path.
fn pinned_go_auth_key(store: &str, provider: &str) -> Option<String> {
    let store: serde_json::Value = serde_json::from_str(store).ok()?;
    let credential = store.get(provider)?;
    (credential.get("type")?.as_str()? == "api")
        .then(|| credential.get("key")?.as_str().map(str::to_owned))?
}

fn assert_workflow_contract(
    config: &toml::Value,
    workflow_id: &str,
    original: &[(&str, &str)],
) -> Result<(), String> {
    let workflow = workflow(config, workflow_id);
    for (node_id, agent) in original {
        if text(node(workflow, node_id), "agent") != *agent {
            return Err(format!(
                "{workflow_id} must retain {node_id}'s {agent} lens"
            ));
        }
    }
    if text(node(workflow, "opencode"), "agent") != "opencode-go" {
        return Err(format!(
            "{workflow_id} must have an independent OpenCode node"
        ));
    }
    let synth_inputs = node_inputs(node(workflow, "synth"));
    if !synth_inputs.contains(&"opencode") {
        return Err(format!(
            "{workflow_id} must pass the independent OpenCode report to synthesis"
        ));
    }
    Ok(())
}

fn assert_config_contract(raw: &str) -> Result<(), String> {
    let config = raw
        .parse::<toml::Value>()
        .map_err(|error| error.to_string())?;
    let agent = array(&config, "agents")
        .iter()
        .find(|agent| text(agent, "id") == "opencode-go")
        .ok_or("missing opencode-go reader")?;
    if text(agent, "cmd") != "opencode"
        || array(agent, "args")
            .iter()
            .map(toml::Value::as_str)
            .collect::<Option<Vec<_>>>()
            != Some(vec!["acp"])
        || agent
            .get("pre_authenticated")
            .and_then(toml::Value::as_bool)
            != Some(true)
        || text(agent, "model") != "opencode-go/kimi-k3"
    {
        return Err("OpenCode reader command or model contract changed".into());
    }
    let sandbox = table(agent, "sandbox");
    for (field, expected) in [
        ("image", "a2a-agent-reader:latest"),
        ("mount", "/Users/wesleyjinks/code"),
        ("access", "ro"),
        ("egress", "locked"),
        ("network", "a2a-egress-internal"),
        ("proxy", "http://a2a-egress-proxy:8888"),
        ("no_proxy", "localhost,127.0.0.1,::1"),
    ] {
        if sandbox.get(field).and_then(toml::Value::as_str) != Some(expected) {
            return Err(format!("OpenCode reader {field} must be {expected}"));
        }
    }
    let volumes = sandbox
        .get("volumes")
        .and_then(toml::Value::as_array)
        .ok_or("OpenCode reader must have a credential volume")?;
    if volumes
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<Vec<_>>()
        != [OPENCODE_VOLUME]
    {
        return Err("OpenCode reader may mount only its read-only credential artifact".into());
    }
    if raw.contains("OPENCODE_API_KEY =") || raw.contains("key = \"") {
        return Err("OpenCode credential must not be a configuration literal".into());
    }
    assert_workflow_contract(&config, "code-review-oc", &[("review", "codex")])?;
    assert_workflow_contract(
        &config,
        "spec-review-oc",
        &[("rigordraft", "codex"), ("soundnessdraft", "claude")],
    )?;
    assert_workflow_contract(
        &config,
        "plan-review-oc",
        &[("execdraft", "codex"), ("coveragedraft", "claude")],
    )?;
    Ok(())
}

fn activated_writer_config(raw: &str) -> String {
    let (prefix, rest) = raw
        .split_once("# [[agents]]\n# id   = \"opencode-go-impl\"")
        .unwrap();
    let (writer, suffix) = rest.split_once("\n# ── impl:").unwrap();
    let writer = format!("# [[agents]]\n# id   = \"opencode-go-impl\"{writer}")
        .lines()
        .map(|line| line.strip_prefix("# ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{prefix}{writer}\n# ── impl:{suffix}")
}

fn assert_writer_contract(raw: &str) -> Result<(), String> {
    let config = raw
        .parse::<toml::Value>()
        .map_err(|error| error.to_string())?;
    let writer = array(&config, "agents")
        .iter()
        .find(|agent| text(agent, "id") == "opencode-go-impl")
        .ok_or("missing activated OpenCode writer")?;
    let sandbox = table(writer, "sandbox");
    if text(writer, "kind") != "container_rw"
        || sandbox.get("access").and_then(toml::Value::as_str) != Some("rw")
        || sandbox.get("image").and_then(toml::Value::as_str) != Some("a2a-toolchain:latest")
        || sandbox.get("no_proxy").and_then(toml::Value::as_str) != Some("localhost,127.0.0.1,::1")
    {
        return Err("activated OpenCode writer lost its writable toolchain contract".into());
    }
    Ok(())
}

#[test]
fn reader_image_and_proxy_pin_the_evidenced_opencode_contract() {
    let root = repo_root();
    let reader = fs::read_to_string(root.join("deploy/containers/reader.Containerfile")).unwrap();
    let toolchain =
        fs::read_to_string(root.join("deploy/containers/toolchain.Containerfile")).unwrap();
    assert!(reader.contains("opencode-ai@1.18.27"));
    assert!(reader.contains("test \"$(opencode --version)\" = \"1.18.27\""));
    assert!(reader.contains("io.a2a-bridge.provenance.opencode.agent-cli=\"opencode-ai=1.18.27\""));
    assert_eq!(
        reader.matches("opencode-ai@").count(),
        1,
        "one immutable installation pin"
    );
    assert_eq!(
        reader
            .matches("io.a2a-bridge.provenance.opencode.agent-cli=")
            .count(),
        1,
        "one provenance label"
    );
    assert!(
        !toolchain.contains("opencode-ai@"),
        "toolchain inherits the reader installation"
    );

    let filter = root.join("deploy/containers/tinyproxy.filter");
    assert!(filter_allows(&filter, "opencode.ai"));
    assert!(filter_allows(&filter, "models.opencode.ai"));
    for denied in [
        "evilopencode.ai",
        "x.opencode.ai",
        "opencode.ai.attacker.net",
        "models.opencode.ai.evil.com",
    ] {
        assert!(!filter_allows(&filter, denied), "filter must deny {denied}");
    }
}

#[test]
fn containerized_opencode_reader_and_review_graphs_are_constrained() {
    let raw = container_config();
    assert_config_contract(&raw).unwrap();
    for path in config_paths() {
        let config = fs::read_to_string(&path).unwrap();
        assert_config_contract(&config)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert_writer_contract(&activated_writer_config(&config))
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }

    let (prefix, opencode_block) = raw.split_once("id   = \"opencode-go\"").unwrap();
    let missing_read_only = format!(
        "{prefix}id   = \"opencode-go\"{}",
        opencode_block.replacen("access  = \"ro\"", "access  = \"rw\"", 1)
    );
    assert!(assert_config_contract(&missing_read_only).is_err());

    let widened_egress = format!(
        "{prefix}id   = \"opencode-go\"{}",
        opencode_block.replacen("egress  = \"locked\"", "egress  = \"open\"", 1)
    );
    assert!(assert_config_contract(&widened_egress).is_err());

    let secret_literal = raw.replacen(
        "model = \"opencode-go/kimi-k3\"",
        "model = \"opencode-go/kimi-k3\"\nkey = \"dummy-go-key\"",
        1,
    );
    assert!(assert_config_contract(&secret_literal).is_err());

    let missing_synth_dependency = raw.replacen(
        "inputs = [\"review\", \"opencode\"]",
        "inputs = [\"review\"]",
        1,
    );
    assert!(assert_config_contract(&missing_synth_dependency).is_err());

    for (input, without_opencode) in [
        (
            "inputs = [\"review\", \"opencode\"]",
            "inputs = [\"review\"]",
        ),
        (
            "inputs = [\"rigor\", \"soundness\", \"opencode\"]",
            "inputs = [\"rigor\", \"soundness\"]",
        ),
        (
            "inputs = [\"exec\", \"coverage\", \"opencode\"]",
            "inputs = [\"exec\", \"coverage\"]",
        ),
    ] {
        assert!(assert_config_contract(&raw.replacen(input, without_opencode, 1)).is_err());
    }

    for path in config_paths() {
        for workflow in ["code-review-oc", "spec-review-oc", "plan-review-oc"] {
            assert_synth_renders_dependencies(&path, workflow)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        }
    }

    let writer = activated_writer_config(&raw);
    assert_writer_contract(&writer).unwrap();
    assert!(
        assert_writer_contract(&writer.replacen("access  = \"rw\"", "access  = \"ro\"", 1))
            .is_err()
    );

    let fixture = tempfile::tempdir().unwrap();
    let config = fixture.path().join("opencode-writer.toml");
    fs::write(
        &config,
        writer.replace(
            "../prompts/",
            &format!("{}/prompts/", repo_root().display()),
        ),
    )
    .unwrap();
    let validation = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["validate", "--config"])
        .arg(config)
        .output()
        .unwrap();
    assert!(
        validation.status.success(),
        "{}",
        String::from_utf8_lossy(&validation.stderr)
    );
}

#[test]
fn opencode_sync_writes_only_the_private_store_and_preserves_its_inode() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("creds");
    let script = repo_root().join("deploy/containers/sync-creds.sh");
    let first_key = "dummy-go-key-\"escaped\"";
    let first = Command::new("bash")
        .arg(&script)
        .arg("opencode")
        .env("A2A_CREDS_DEST", &store)
        .env("OPENCODE_API_KEY", first_key)
        .output()
        .unwrap();
    assert!(first.status.success());
    let first_output = format!(
        "{}{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(!first_output.contains(first_key));

    let credential = store.join("opencode/auth.json");
    let metadata = fs::metadata(&credential).unwrap();
    let inode = metadata.ino();
    assert_eq!(metadata.mode() & 0o777, 0o600);
    let first_store = fs::read_to_string(&credential).unwrap();
    for provider in ["opencode-go", "opencode"] {
        assert_eq!(
            pinned_go_auth_key(&first_store, provider).as_deref(),
            Some(first_key),
            "credential store must retain the {provider} record"
        );
    }
    assert_eq!(
        pinned_go_auth_key(
            r#"{"opencode":{"type":"api","key":"wrong-provider"}}"#,
            "opencode-go"
        ),
        None,
        "the Go provider record must be present"
    );

    let second_key = "dummy-go-key-two";
    let second = Command::new("bash")
        .arg(&script)
        .arg("opencode")
        .env("A2A_CREDS_DEST", &store)
        .env("OPENCODE_API_KEY", second_key)
        .output()
        .unwrap();
    assert!(second.status.success());
    let second_output = format!(
        "{}{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(!second_output.contains(second_key));
    assert_eq!(fs::metadata(&credential).unwrap().ino(), inode);
    let second_store = fs::read_to_string(&credential).unwrap();
    for provider in ["opencode-go", "opencode"] {
        assert_eq!(
            pinned_go_auth_key(&second_store, provider).as_deref(),
            Some(second_key),
            "in-place refresh must retain the {provider} record"
        );
    }

    let missing_store = temp.path().join("missing");
    let missing = Command::new("bash")
        .arg(script)
        .arg("opencode")
        .env("A2A_CREDS_DEST", &missing_store)
        .env_remove("OPENCODE_API_KEY")
        .output()
        .unwrap();
    assert!(missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stdout).contains("OPENCODE_API_KEY is not set"));
    assert!(!missing_store.join("opencode/auth.json").exists());
}

#[test]
fn routing_reference_keeps_the_dated_go_choices_and_privacy_exclusions() {
    let routing = fs::read_to_string(repo_root().join("docs/compatibility-routing.md")).unwrap();
    for model in [
        "opencode-go/kimi-k3",
        "opencode-go/glm-5.3",
        "opencode-go/qwen3.8-max",
        "opencode-go/grok-4.7",
        "opencode-go/kimi-k2.7-code",
        "opencode-go/glm-5.2",
        "opencode-go/deepseek-v4-pro",
        "opencode-go/minimax-m3",
        "opencode-go/mimo-v2.6-pro",
    ] {
        assert!(routing.contains(model), "routing must retain {model}");
    }
    assert_routing_contract(&routing).unwrap();
    assert!(assert_routing_contract(&routing.replacen(
        "Do not recommend or include Gemini 4 Argon Contributor or Muse Spark Contributor models in examples:",
        "Recommend Gemini 4 Argon Contributor and Muse Spark Contributor models in examples:",
        1,
    ))
    .is_err());
    assert!(assert_routing_contract(&routing.replacen(
        "| `opencode-go/kimi-k3` | Independent bounded review |",
        "| `opencode-go/kimi-k3` | Unbounded implementation |",
        1,
    ))
    .is_err());
}

fn assert_routing_contract(routing: &str) -> Result<(), String> {
    for required in [
        "Gemini 4 Argon Contributor",
        "Muse Spark Contributor",
        "training on prompts and completions",
        "Do not recommend or include",
        "| `opencode-go/kimi-k3` | Independent bounded review |",
        "| `opencode-go/kimi-k2.7-code` | Implementation candidate |",
    ] {
        if !routing.contains(required) {
            return Err(format!("missing routing restriction: {required}"));
        }
    }
    Ok(())
}
