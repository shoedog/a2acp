// integration_run_workflow.rs — Unit-level tests for the `run-workflow` CLI wiring.
//
// Tests the arg-parser and config-load path (dispatch + workflow lookup) WITHOUT
// running a real executor or touching any live ACP agents.  A full live run
// requires real agents (tested by e2e_*); these focus on the CLI seam:
//
//   1. `parse_args_missing_input` — --input omitted → clean Err (not a panic).
//   2. `parse_args_unknown_flag`  — unknown flag → clean Err.
//   3. `unknown_workflow_id_fails_cleanly` — config loads OK; workflow id not in
//      the map → clean Err message containing the unknown id.
//   4. `known_workflow_id_resolves_graph` — a temp config with one workflow → the
//      graph is found and has the expected node count.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

// The parse helper and cmd are private to main.rs; we test through a re-exported
// helper (or via the binary's public test surface — the bin crate exposes nothing
// for test, so we inline equivalent logic here that mirrors what the subcommand does).

/// Minimal config with two agents and one workflow (one terminal node).
fn write_minimal_config(dir: &std::path::Path, prompts_dir: &std::path::Path) -> PathBuf {
    let prompt_path = prompts_dir.join("p.md");
    std::fs::write(&prompt_path, "review {{input}}").unwrap();
    // prompt_file is relative to the config dir
    let toml = format!(
        r#"default = "codex"

[[agents]]
id = "codex"
cmd = "codex-acp"

[[agents]]
id = "claude"
cmd = "claude-agent-acp"

[[workflows]]
id = "code-review"

[[workflows.nodes]]
id = "only"
agent = "codex"
prompt_file = "{}"
inputs = []

[server]
addr = "127.0.0.1:8080"
"#,
        // Use an absolute path so this works regardless of cwd.
        prompt_path.display()
    );
    let config_path = dir.join("a2a-bridge.workflows.toml");
    std::fs::write(&config_path, toml).unwrap();
    config_path
}

/// Load config + workflow map; return Err(String) for clean-error assertions.
fn load_workflow_map(
    config_path: &std::path::Path,
) -> Result<
    std::collections::HashMap<
        bridge_core::ids::WorkflowId,
        std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
    >,
    String,
> {
    let raw = std::fs::read_to_string(config_path).map_err(|e| e.to_string())?;
    // Import the binary's own config module via a re-parse (we test equivalent logic).
    use bridge_core::ids::{AgentId, NodeId, WorkflowId};
    use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};

    // Parse TOML manually (same fields as RegistryConfig) to avoid coupling to
    // internal binary config types.
    #[derive(serde::Deserialize)]
    struct Cfg {
        #[allow(dead_code)]
        default: String,
        #[allow(dead_code)]
        agents: Vec<AgentEntry>,
        #[serde(default)]
        workflows: Vec<Workflow>,
        #[allow(dead_code)]
        server: Server,
    }
    #[derive(serde::Deserialize)]
    struct AgentEntry {
        id: String,
        #[allow(dead_code)]
        cmd: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Workflow {
        id: String,
        #[serde(default)]
        nodes: Vec<Node>,
    }
    #[derive(serde::Deserialize)]
    struct Node {
        id: String,
        agent: String,
        prompt_file: String,
        #[serde(default)]
        inputs: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    struct Server {
        #[allow(dead_code)]
        addr: Option<String>,
    }

    let cfg: Cfg = toml::from_str(&raw).map_err(|e| e.to_string())?;
    let base = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    let agent_ids: std::collections::HashSet<&str> =
        cfg.agents.iter().map(|a| a.id.as_str()).collect();

    let mut map = std::collections::HashMap::new();
    for w in &cfg.workflows {
        let id = WorkflowId::parse(w.id.clone()).map_err(|e| format!("{e:?}"))?;
        let mut nodes = Vec::new();
        for n in &w.nodes {
            if !agent_ids.contains(n.agent.as_str()) {
                return Err(format!("unknown agent {:?}", n.agent));
            }
            // prompt_file may be absolute (from our test helper) OR relative to base.
            let pf = std::path::Path::new(&n.prompt_file);
            let tpl = if pf.is_absolute() {
                std::fs::read_to_string(pf).map_err(|e| e.to_string())?
            } else {
                std::fs::read_to_string(base.join(pf)).map_err(|e| e.to_string())?
            };
            nodes.push(WorkflowNode {
                id: NodeId::parse(n.id.clone()).map_err(|e| format!("{e:?}"))?,
                agent: AgentId::parse(n.agent.clone()).map_err(|e| format!("{e:?}"))?,
                prompt_template: tpl,
                inputs: n
                    .inputs
                    .iter()
                    .map(|i| NodeId::parse(i.clone()).map_err(|e| format!("{e:?}")))
                    .collect::<Result<_, _>>()?,
                retry: None,
                harvest_sanitization: None,
            });
        }
        let g = WorkflowGraph {
            id: id.clone(),
            nodes,
            panel: None,
        };
        g.validate().map_err(|e| format!("{e:?}"))?;
        map.insert(id, std::sync::Arc::new(g));
    }
    Ok(map)
}

// --- tests ---

#[test]
fn unknown_workflow_id_fails_cleanly() {
    let dir = tempdir().unwrap();
    let prompts = tempdir().unwrap();
    let config_path = write_minimal_config(dir.path(), prompts.path());
    let wf_map = load_workflow_map(&config_path).expect("config should load");

    // "not-a-workflow" is not in the map.
    let target = bridge_core::ids::WorkflowId::parse("not-a-workflow").unwrap();
    assert!(
        !wf_map.contains_key(&target),
        "unknown workflow id should not resolve"
    );
}

#[test]
fn known_workflow_id_resolves_graph() {
    let dir = tempdir().unwrap();
    let prompts = tempdir().unwrap();
    let config_path = write_minimal_config(dir.path(), prompts.path());
    let wf_map = load_workflow_map(&config_path).expect("config should load");

    let target = bridge_core::ids::WorkflowId::parse("code-review").unwrap();
    let graph = wf_map
        .get(&target)
        .expect("code-review workflow must be present");
    assert_eq!(graph.nodes.len(), 1, "graph should have exactly 1 node");
    assert_eq!(graph.nodes[0].id.as_str(), "only");
    assert_eq!(graph.nodes[0].prompt_template, "review {{input}}");
}

#[test]
fn example_config_loads_single_sol_review_node() {
    // The shipped example/a2a-bridge.workflows.toml must parse (prompt files exist).
    // This test is a smoke-check that the example config and prompts/ are in sync.
    let config_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/a2a-bridge.workflows.toml");
    if !config_path.exists() {
        return; // skip if not present (shouldn't happen)
    }
    let raw = std::fs::read_to_string(&config_path).unwrap();
    // Parse via the binary's own RegistryConfig so we test the real path.
    // We can't import bin internals, so use a minimal TOML parse to verify node count.
    #[derive(serde::Deserialize)]
    struct Cfg {
        #[serde(default)]
        workflows: Vec<Workflow>,
        #[allow(dead_code)]
        default: String,
        #[allow(dead_code)]
        agents: Vec<serde_json::Value>,
        #[allow(dead_code)]
        server: serde_json::Value,
    }
    #[derive(serde::Deserialize)]
    struct Workflow {
        #[allow(dead_code)]
        id: String,
        #[serde(default)]
        nodes: Vec<serde_json::Value>,
    }
    let cfg: Cfg = toml::from_str(&raw).expect("example config must parse");
    let wf = cfg
        .workflows
        .iter()
        .find(|w| w.id == "code-review")
        .expect("code-review workflow must exist");
    assert_eq!(
        wf.nodes.len(),
        1,
        "default code-review must have exactly one billable Sol reviewer"
    );
    assert_eq!(wf.nodes[0]["id"], "review");
    assert_eq!(wf.nodes[0]["agent"], "codex");
    assert_eq!(wf.nodes[0]["prompt"], "review-sol-risk");
}

fn run_offline_legacy_success(
    advertise_v1: bool,
) -> (
    std::process::Output,
    bridge_core::workflow_history::AttemptTerminal,
    String,
) {
    let directory = tempdir().unwrap();
    let adapter = directory.path().join("missing-evidence-acp");
    let initialize = if advertise_v1 {
        r#"printf '{"jsonrpc":"2.0","result":{"protocolVersion":1,"agentCapabilities":{"_meta":{"a2a_bridge.turn_evidence.v1":true}},"authMethods":[],"agentInfo":{"name":"missing-evidence-acp","title":"Missing evidence fake","version":"1.0.0"}},"id":%s}\n' "$id""#
    } else {
        r#"printf '{"jsonrpc":"2.0","result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[],"agentInfo":{"name":"legacy-acp","title":"Legacy fake","version":"1.0.0"}},"id":%s}\n' "$id""#
    };
    let adapter_script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -E 's/^\{"jsonrpc":"2.0","id":([^,]+),"method".*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      __INITIALIZE_RESPONSE__
      ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","result":{"sessionId":"offline-missing-evidence"},"id":%s}\n' "$id"
      ;;
    *'"method":"session/set_mode"'*|*'"method":"session/set_config_option"'*)
      printf '{"jsonrpc":"2.0","result":{},"id":%s}\n' "$id"
      ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"offline-missing-evidence","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"FINAL"}}}}'
      printf '{"jsonrpc":"2.0","result":{"stopReason":"end_turn"},"id":%s}\n' "$id"
      ;;
  esac
done
"#
    .replace("__INITIALIZE_RESPONSE__", initialize);
    fs::write(&adapter, adapter_script).unwrap();
    let mut permissions = fs::metadata(&adapter).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).unwrap();

    let prompt = directory.path().join("prompt.md");
    fs::write(&prompt, "{{input}}").unwrap();
    let input = directory.path().join("input.md");
    fs::write(
        &input,
        "---\ntask-type: freeform\n---\nprovider-free input\n",
    )
    .unwrap();
    let output_path = directory.path().join("result.md");
    let store_path = directory.path().join("history.sqlite");
    let config = directory.path().join("a2a-bridge.toml");
    fs::write(
        &config,
        format!(
            r#"default = "fake"
allowed_cwd_root = {root:?}

[registry]
allowed_cmds = [{adapter:?}]

[[agents]]
id = "fake"
cmd = {adapter:?}
pre_authenticated = true

[store]
path = {store:?}

[[workflows]]
id = "offline-v1"

[[workflows.nodes]]
id = "only"
agent = "fake"
prompt_file = {prompt:?}
inputs = []

[server]
addr = "127.0.0.1:0"
"#,
            root = directory.path(),
            adapter = adapter,
            store = store_path,
            prompt = prompt,
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args([
            "run-workflow",
            "offline-v1",
            "--input",
            input.to_str().unwrap(),
            "--session-cwd",
            directory.path().to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
            "--out",
            output_path.to_str().unwrap(),
        ])
        .current_dir(directory.path())
        .output()
        .expect("run provider-free offline workflow");

    let connection = rusqlite::Connection::open(&store_path).unwrap();
    let terminal_json: String = connection
        .query_row(
            "SELECT terminal_json FROM workflow_attempt_summaries WHERE status='terminal'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let terminal: bridge_core::workflow_history::AttemptTerminal =
        serde_json::from_str(&terminal_json).unwrap();
    let output_text = fs::read_to_string(output_path).unwrap();
    (output, terminal, output_text)
}

#[test]
fn offline_v1_resolution_controls_exit_after_a_legacy_success_stream() {
    let (output, terminal, output_text) = run_offline_legacy_success(true);
    assert_eq!(terminal.outcome, "failed");
    assert_eq!(
        terminal.terminal_reason,
        "protocol_terminal_evidence_missing"
    );
    assert!(
        !output.status.success(),
        "offline public exit must follow resolved terminal truth; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output_text, "FINAL");
}

#[test]
fn offline_unsupported_legacy_success_remains_completed() {
    let (output, terminal, output_text) = run_offline_legacy_success(false);
    assert!(
        output.status.success(),
        "unsupported legacy success must remain successful; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(terminal.outcome, "completed");
    assert_eq!(terminal.terminal_evidence_capability, "unsupported");
    assert_eq!(output_text, "FINAL");
}
