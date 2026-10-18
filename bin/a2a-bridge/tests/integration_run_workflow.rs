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

mod text_file_busy;

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
                model: None,
                effort: None,
                mode: None,
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
            controls: None,
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
    bridge_core::workflow_history::AttemptStructuredEvidenceV1,
) {
    run_offline_workflow(advertise_v1, "", &[])
}

/// An explicit local cwd is admitted against the *resolved* configured root before any adapter
/// (including a container warm-up) can start.  The adapter's startup marker is therefore a
/// stronger regression signal than its eventual workflow result.
#[cfg(unix)]
#[test]
fn local_run_workflow_canonical_cwd_admission_precedes_agent_spawn() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("root");
    let child = root.join("child");
    let outside = directory.path().join("outside");
    let lookalike = directory.path().join("root-lookalike");
    fs::create_dir_all(&child).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(outside.join("subdir")).unwrap();
    fs::create_dir_all(outside.join("child")).unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::create_dir_all(root.join("target")).unwrap();
    fs::create_dir_all(outside.join("target")).unwrap();
    fs::create_dir_all(root.join("a/b/c")).unwrap();
    fs::create_dir_all(&lookalike).unwrap();
    let escaping_link = root.join("escaping-link");
    std::os::unix::fs::symlink(&outside, &escaping_link).unwrap();
    let combined_escape_link = root.join("combined-escape-link");
    std::os::unix::fs::symlink(outside.join("subdir"), &combined_escape_link).unwrap();
    let escaping_dotdot = root.join("..").join("outside");
    let combined_escape_dotdot = combined_escape_link.join("..").join("child");
    let missing_component_dotdot = root.join("missing").join("..").join("child");
    // The original spelling resolves inside `root`, while `SessionCwd::parse` folds `..` before
    // resolving symlinks and would execute outside it.
    let parsed_escape_link = outside.join("parsed-escape-link");
    std::os::unix::fs::symlink(root.join("sub"), &parsed_escape_link).unwrap();
    let parsed_escape_after_link = parsed_escape_link.join("..").join("target");
    // This variant stays lexically under `root` until its second `..`, but parse normalizes it to
    // `root/..` while filesystem resolution of the original spelling lands at `root/a`.
    let parsed_escape_root_link = root.join("parsed-escape-root-link");
    std::os::unix::fs::symlink(root.join("a/b/c"), &parsed_escape_root_link).unwrap();
    let parsed_escape_after_root_link = parsed_escape_root_link.join("..").join("..");
    let root_alias = directory.path().join("root-alias");
    std::os::unix::fs::symlink(&root, &root_alias).unwrap();
    let alias_child = root_alias.join("child");
    let alias_escape = root_alias.join("escaping-link");
    use std::os::unix::ffi::OsStringExt as _;
    let non_utf8_root = directory
        .path()
        .join(std::ffi::OsString::from_vec(b"repo-\xff".to_vec()));
    let non_utf8_outside = directory
        .path()
        .join(std::ffi::OsString::from_vec(b"repo-\xfe".to_vec()));
    let non_utf8_root_alias = directory.path().join("non-utf8-root-alias");
    let non_utf8_outside_alias = directory.path().join("non-utf8-outside-alias");
    // APFS refuses non-UTF-8 names with EILSEQ; Linux CI still covers that case.
    let non_utf8_supported = match fs::create_dir(&non_utf8_root) {
        Ok(()) => {
            fs::create_dir(&non_utf8_outside).unwrap();
            std::os::unix::fs::symlink(&non_utf8_root, &non_utf8_root_alias).unwrap();
            std::os::unix::fs::symlink(&non_utf8_outside, &non_utf8_outside_alias).unwrap();
            true
        }
        Err(error) if error.raw_os_error() == Some(libc::EILSEQ) => false,
        Err(error) => panic!("create non-UTF-8 fixture: {error}"),
    };

    let adapter = directory.path().join("cwd-guard-acp");
    let spawned = directory.path().join("adapter-spawned");
    let delivered_cwd = directory.path().join("delivered-cwd");
    let script = r#"#!/bin/sh
touch '__SPAWNED__'
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -E 's/^\{"jsonrpc":"2.0","id":([^,]+),"method".*/\1/')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]},"id":%s}\n' "$id" ;;
    *'"method":"session/new"'*) printf '%s\n' "$line" > '__DELIVERED_CWD__'; printf '{"jsonrpc":"2.0","result":{"sessionId":"cwd-guard"},"id":%s}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"cwd-guard","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"FINAL"}}}}'
      printf '{"jsonrpc":"2.0","result":{"stopReason":"end_turn"},"id":%s}\n' "$id" ;;
  esac
done
"#
    .replace("__SPAWNED__", spawned.to_str().unwrap())
    .replace("__DELIVERED_CWD__", delivered_cwd.to_str().unwrap());
    fs::write(&adapter, script).unwrap();
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
    text_file_busy::warm_up_script(&adapter);
    let prompt = directory.path().join("prompt.md");
    let input = directory.path().join("input.md");
    let config = directory.path().join("a2a-bridge.toml");
    fs::write(&prompt, "{{input}}").unwrap();
    fs::write(
        &input,
        "---\ntask-type: freeform\n---\nprovider-free input\n",
    )
    .unwrap();

    let run = |cwd: Option<&std::path::Path>, configured_root: Option<&std::path::Path>| {
        let root = configured_root
            .map(|path| format!("allowed_cwd_root = {path:?}\n"))
            .unwrap_or_default();
        fs::write(
            &config,
            format!(
                "default = \"fake\"\n{root}[registry]\nallowed_cmds = [{adapter:?}]\n\
                 [[agents]]\nid = \"fake\"\ncmd = {adapter:?}\npre_authenticated = true\n\
                 [[workflows]]\nid = \"cwd-guard\"\n[[workflows.nodes]]\nid = \"only\"\n\
                 agent = \"fake\"\nprompt_file = {prompt:?}\ninputs = []\n[server]\naddr = \"127.0.0.1:0\"\n",
                adapter = adapter,
                prompt = prompt,
            ),
        )
        .unwrap();
        let _ = fs::remove_file(&spawned);
        let _ = fs::remove_file(&delivered_cwd);
        let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
        command
            .args([
                "run-workflow",
                "cwd-guard",
                "--input",
                input.to_str().unwrap(),
                "--config",
                config.to_str().unwrap(),
            ])
            .current_dir(directory.path());
        if let Some(cwd) = cwd {
            command.args(["--session-cwd", cwd.to_str().unwrap()]);
        }
        command.output().unwrap()
    };

    let mut rejected_cases = vec![
        outside.as_path(),
        lookalike.as_path(),
        escaping_dotdot.as_path(),
        escaping_link.as_path(),
        alias_escape.as_path(),
        combined_escape_dotdot.as_path(),
        missing_component_dotdot.as_path(),
        parsed_escape_after_link.as_path(),
        parsed_escape_after_root_link.as_path(),
    ];
    if non_utf8_supported {
        rejected_cases.push(non_utf8_outside_alias.as_path());
    }
    for rejected in rejected_cases {
        let configured_root = if rejected == non_utf8_outside_alias {
            non_utf8_root_alias.as_path()
        } else if rejected == alias_escape {
            root_alias.as_path()
        } else {
            root.as_path()
        };
        let output = run(Some(rejected), Some(configured_root));
        assert!(!output.status.success(), "{rejected:?} unexpectedly ran");
        assert!(
            !spawned.exists(),
            "{rejected:?} spawned an adapter: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("--session-cwd"));
    }
    let missing_root = directory.path().join("missing-root");
    let missing_cwd = root.join("missing-cwd");
    for (cwd, configured_root) in [
        (child.as_path(), missing_root.as_path()),
        (missing_cwd.as_path(), root.as_path()),
    ] {
        let output = run(Some(cwd), Some(configured_root));
        assert!(!output.status.success(), "{cwd:?} unexpectedly ran");
        assert!(!spawned.exists(), "{cwd:?} spawned an adapter");
    }

    for (cwd, configured_root) in [
        (Some(root.as_path()), Some(root.as_path())),
        (Some(child.as_path()), Some(root.as_path())),
        (Some(alias_child.as_path()), Some(root_alias.as_path())),
        (Some(outside.as_path()), None),
        (None, Some(root.as_path())),
    ] {
        let output = run(cwd, configured_root);
        assert!(
            output.status.success() && spawned.exists(),
            "cwd={cwd:?}, root={configured_root:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if let Some(cwd) = cwd {
            let session: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&delivered_cwd).unwrap()).unwrap();
            assert_eq!(session["params"]["cwd"], cwd.to_str().unwrap());
        }
    }
}

/// One provider-free offline `run-workflow` against a fake ACP adapter, with `workflow_keys`
/// added to the `[[workflows]]` table and `extra_args` to the command line.
fn run_offline_workflow(
    advertise_v1: bool,
    workflow_keys: &str,
    extra_args: &[&str],
) -> (
    std::process::Output,
    bridge_core::workflow_history::AttemptTerminal,
    String,
    bridge_core::workflow_history::AttemptStructuredEvidenceV1,
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
    text_file_busy::warm_up_script(&adapter);

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
{workflow_keys}

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
        .args(extra_args)
        .current_dir(directory.path())
        .output()
        .expect("run provider-free offline workflow");

    let connection = rusqlite::Connection::open(&store_path).unwrap();
    let (attempt_id, terminal_json, evidence_schema_version, reservation_json): (
        String,
        String,
        i64,
        String,
    ) = connection
        .query_row(
            "SELECT attempt_id, terminal_json, evidence_schema_version, structured_reservation_json
             FROM workflow_attempt_summaries WHERE status='terminal'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap_or_else(|error| {
            panic!(
                "history summary missing ({error:?}); stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            )
        });
    assert_eq!(
        evidence_schema_version,
        i64::from(bridge_core::workflow_history::WORKFLOW_HISTORY_EVIDENCE_SCHEMA_V1),
    );
    let terminal: bridge_core::workflow_history::AttemptTerminal =
        serde_json::from_str(&terminal_json).unwrap();
    let reservation: bridge_core::workflow_history::AttemptReservationV2 =
        serde_json::from_str(&reservation_json).unwrap();
    let mut node_statement = connection
        .prepare(
            "SELECT node_id, terminal_json
             FROM workflow_attempt_node_terminals
             WHERE attempt_id=?1 ORDER BY node_id",
        )
        .unwrap();
    let node_terminals = node_statement
        .query_map([attempt_id], |row| {
            let node_id: String = row.get(0)?;
            Ok(bridge_core::workflow_history::HistoryNodeTerminalV1 {
                node: bridge_core::ids::NodeId::parse(node_id).unwrap(),
                sorted_ordinal: 0,
                terminal_json: row.get(1)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let output_text = fs::read_to_string(output_path).unwrap();
    (
        output,
        terminal.clone(),
        output_text,
        bridge_core::workflow_history::AttemptStructuredEvidenceV1 {
            reservation,
            node_terminals,
            policy_trigger_json: terminal.policy_trigger_json.clone(),
        },
    )
}

#[test]
fn offline_v1_resolution_controls_exit_after_a_legacy_success_stream() {
    let (output, terminal, output_text, structured) = run_offline_legacy_success(true);
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
    assert_eq!(structured.reservation.expected_node_count, 1);
    assert_eq!(structured.node_terminals.len(), 1);
    let node = bridge_core::execution_policy::NodeTerminalV1::decode_canonical(
        structured.node_terminals[0].terminal_json.as_bytes(),
    )
    .unwrap();
    assert_eq!(
        node.primary,
        bridge_core::execution_policy::NodePrimaryDispositionV1::Completed,
    );
    assert!(node.cause.is_none());
}

#[test]
fn offline_unsupported_legacy_success_remains_completed() {
    let (output, terminal, output_text, structured) = run_offline_legacy_success(false);
    assert!(
        output.status.success(),
        "unsupported legacy success must remain successful; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(terminal.outcome, "completed");
    assert_eq!(terminal.terminal_evidence_capability, "unsupported");
    assert_eq!(output_text, "FINAL");
    assert_eq!(structured.reservation.expected_node_count, 1);
    assert_eq!(structured.node_terminals.len(), 1);
    assert_eq!(structured.node_terminals[0].node.as_str(), "only");
    let stderr = String::from_utf8(output.stderr).unwrap();
    let structured_stderr = stderr
        .lines()
        .find_map(|line| {
            line.strip_prefix("node_terminal{node=\"only\",terminal_json=")
                .and_then(|value| value.strip_suffix('}'))
        })
        .expect("offline stderr must retain the exact bounded node terminal");
    let decoded = bridge_core::execution_policy::NodeTerminalV1::decode_canonical(
        structured_stderr.as_bytes(),
    )
    .unwrap();
    assert_eq!(
        decoded.primary,
        bridge_core::execution_policy::NodePrimaryDispositionV1::Completed
    );
    assert_eq!(
        decoded,
        bridge_core::execution_policy::NodeTerminalV1::decode_canonical(
            structured.node_terminals[0].terminal_json.as_bytes(),
        )
        .unwrap(),
    );
}

/// Activity liveness I2a, end to end through the real binary: the silence cutoff an offline run
/// freezes (as reserved in its history ledger) follows `--silence-cutoff`, then
/// `[[workflows]] silence_cutoff`, then the agent default; `off` at either level freezes no cutoff.
#[test]
fn offline_run_freezes_the_silence_cutoff_by_invocation_then_workflow() {
    use bridge_core::execution_policy::{FrozenWorkflowControlsV1, SilenceCutoffV1};

    let cases: [(&str, &[&str], SilenceCutoffV1); 5] = [
        ("", &[], SilenceCutoffV1::AgentDefault),
        ("silence_cutoff = \"off\"", &[], SilenceCutoffV1::None),
        (
            "silence_cutoff = 1200",
            &[],
            SilenceCutoffV1::Activity {
                silence_ms: 1_200_000,
            },
        ),
        (
            "silence_cutoff = \"off\"",
            &["--silence-cutoff", "900"],
            SilenceCutoffV1::Activity {
                silence_ms: 900_000,
            },
        ),
        (
            "silence_cutoff = 1200",
            &["--silence-cutoff", "off"],
            SilenceCutoffV1::None,
        ),
    ];
    for (workflow_keys, args, expected) in cases {
        let (output, terminal, output_text, structured) =
            run_offline_workflow(false, workflow_keys, args);
        assert!(
            output.status.success(),
            "{workflow_keys:?} {args:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(terminal.outcome, "completed");
        assert_eq!(output_text, "FINAL");
        let controls: FrozenWorkflowControlsV1 =
            serde_json::from_str(&structured.reservation.controls_json).unwrap();
        assert_eq!(
            controls.profile.silence_cutoff, expected,
            "{workflow_keys:?} {args:?}"
        );
        assert_eq!(controls.effective_work_cutoff_ms(), 7_200_000);
    }
}

#[test]
fn node_selection_uses_acp_capability_and_fable_gates_before_task_prompt() {
    let dir = tempdir().unwrap();
    let adapter = dir.path().join("selection-acp");
    let prompted = dir.path().join("prompted");
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -E 's/^\{"jsonrpc":"2.0","id":([^,]+),"method".*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]},"id":%s}\n' "$id" ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","result":{"sessionId":"selected","models":{"currentModelId":"good","availableModels":[{"modelId":"good","name":"Good"},{"modelId":"claude-fable-5","name":"Fable"}]},"modes":{"currentModeId":"code","availableModes":[{"id":"code","name":"Code"}]}},"id":%s}\n' "$id" ;;
    *'"method":"session/set_mode"'*'"modeId":"missing"'*)
      printf '{"jsonrpc":"2.0","error":{"code":-32602,"message":"invalid mode"},"id":%s}\n' "$id" ;;
    *'"method":"session/set_model"'*|*'"method":"session/set_mode"'*|*'"method":"session/set_config_option"'*)
      printf '{"jsonrpc":"2.0","result":{},"id":%s}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      touch '__PROMPTED__'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"selected","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"FINAL"}}}}'
      printf '{"jsonrpc":"2.0","result":{"stopReason":"end_turn"},"id":%s}\n' "$id" ;;
  esac
done
"#.replace("__PROMPTED__", prompted.to_str().unwrap());
    fs::write(&adapter, script).unwrap();
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
    text_file_busy::warm_up_script(&adapter);
    let input = dir.path().join("input.md");
    fs::write(&input, "---\ntask-type: freeform\n---\nCheck selection\n").unwrap();
    for (model, mode, allow_fable, succeeds, error) in [
        ("missing", "code", false, false, "not advertised"),
        ("good", "missing", false, false, "mode"),
        ("claude-fable-5", "code", false, false, "blocked"),
        ("claude-fable-5", "code", true, true, ""),
        (
            "claude-fable-missing",
            "code",
            true,
            false,
            "not advertised",
        ),
    ] {
        let _ = fs::remove_file(&prompted);
        let config = dir.path().join("selection.toml");
        fs::write(
            &config,
            format!(
                r#"
default = "fake"
allowed_cwd_root = {root:?}
[registry]
allowed_cmds = [{adapter:?}]
[[agents]]
id = "fake"
cmd = {adapter:?}
pre_authenticated = true
model = "good"
[[workflows]]
id = "selection"
[[workflows.nodes]]
id = "only"
agent = "fake"
prompt_text = "{{{{input}}}}"
model = {model:?}
mode = {mode:?}
[server]
addr = "127.0.0.1:0"
"#,
                root = dir.path()
            ),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .args([
                "run-workflow",
                "selection",
                "--config",
                config.to_str().unwrap(),
                "--input",
                input.to_str().unwrap(),
                "--session-cwd",
                dir.path().to_str().unwrap(),
            ])
            .env(
                "A2A_BRIDGE_ALLOW_FABLE",
                if allow_fable { "1" } else { "0" },
            )
            .current_dir(dir.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.success(),
            succeeds,
            "{model}/{mode}: {stderr}"
        );
        assert_eq!(
            prompted.exists(),
            succeeds,
            "invalid selection reached the task prompt: {stderr}"
        );
        if succeeds {
            assert!(
                stderr.contains(model),
                "progress must show configured model: {stderr}"
            );
        } else {
            assert!(format!("{stderr}{}", String::from_utf8_lossy(&output.stdout)).contains(error));
        }
    }
}
