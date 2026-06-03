// main.rs — A2A Bridge v3b composition root (spec §7/§8, Task 12).
//
// Wires all port implementations together into a runnable binary:
//   AlwaysGrant (auth) -> SkillRoute (route) -> Registry -> AcpBackend (backend)
//   AutoPolicy (policy) | SqliteStore (store) | PeerDelegation or StubDelegation (delegation)
//
// The agent registry is driven by a `FileConfigSource` over `a2a-bridge.toml`:
// the initial `load()` is the desired state `Registry::new` VALIDATES (boot fails
// loud on bad config, spec §7), and a detached reconcile loop applies every
// subsequent `watch()` snapshot so on-disk edits hot-reload the live registry.
//
// Server listens on cfg.server.addr (default 127.0.0.1:8080).
//
// Subcommands:
//   a2a-bridge                                           — serve (default)
//   a2a-bridge run-workflow <id> --input <file>
//             [--out <file>] [--config <path>]           — run a workflow offline
//   a2a-bridge submit <skill> --input <file> [--url <url>]
//                                                        — submit a detached task
//   a2a-bridge task get <id> [--url <url>]               — get task by id
//   a2a-bridge task list [--limit <n>] [--url <url>]     — list tasks
//   a2a-bridge task cancel <id> [--url <url>]            — cancel task by id

mod config;
mod route;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bridge_a2a_inbound::server::InboundServer;
use bridge_a2a_outbound::{PeerDelegation, StubDelegation};
use bridge_acp::acp_backend::{AcpBackend, AcpConfig};
use bridge_core::domain::AgentEntry;
use bridge_core::error::BridgeError;
use bridge_core::ports::{AgentBackend, AgentRegistry, ConfigSource, DelegationPort, PolicyEngine};
use bridge_policy::{auth::AlwaysGrant, permission::AutoPolicy};
use bridge_registry::registry::{Registry, SpawnFn};
use bridge_store::sqlite::SqliteStore;
use config::{FileConfigSource, RegistryConfig};
use route::SkillRoute;

/// Path of the on-disk registry config the bridge watches + hot-reloads.
const CONFIG_PATH: &str = "a2a-bridge.toml";

/// Built-in default config (new 3b registry schema) materialised to disk when
/// `a2a-bridge.toml` is absent, so the `FileConfigSource` load/watch pipeline has
/// a concrete path to read and the parent directory to watch. A sensible
/// single-agent default: one `kiro` agent running `kiro-cli acp`.
const DEFAULT_CONFIG: &str = r#"default = "kiro"

[registry]
allowed_cmds = ["kiro-cli"]

[[agents]]
id   = "kiro"
cmd  = "kiro-cli"
args = ["acp"]

[server]
addr = "127.0.0.1:8080"
"#;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Resolve the static (config-time) ACP session cwd for an agent entry.
/// Resolution chain: `session_cwd` → `cwd` → `"."`.
/// The host child has no cwd (Supervised gets None); AcpConfig.cwd IS the ACP session cwd.
fn resolve_static_session_cwd(session_cwd: Option<&str>, cwd: Option<&str>) -> String {
    session_cwd.or(cwd).unwrap_or(".").to_string()
}

// ---------------------------------------------------------------------------
// `run-workflow` subcommand
// ---------------------------------------------------------------------------

/// Parse `a2a-bridge run-workflow <id> --input <file> [--out <file>] [--config <path>]`
/// from a raw args iterator (skipping the binary name at position 0 and the
/// subcommand name at position 1).
fn parse_run_workflow_args(
    args: &[String],
) -> Result<(String, PathBuf, Option<PathBuf>, PathBuf), BoxError> {
    let mut iter = args.iter().peekable();
    // Positional: workflow id
    let workflow_id = iter
        .next()
        .cloned()
        .ok_or("run-workflow: missing <workflow-id>")?;
    let mut input: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut config: Option<PathBuf> = None;
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--input" => {
                input = Some(PathBuf::from(
                    iter.next()
                        .ok_or("run-workflow: --input requires a value")?,
                ));
            }
            "--out" => {
                out = Some(PathBuf::from(
                    iter.next().ok_or("run-workflow: --out requires a value")?,
                ));
            }
            "--config" => {
                config = Some(PathBuf::from(
                    iter.next()
                        .ok_or("run-workflow: --config requires a value")?,
                ));
            }
            other => return Err(format!("run-workflow: unknown flag {other:?}").into()),
        }
    }
    let input = input.ok_or("run-workflow: --input <file> is required")?;
    let config = config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH));
    Ok((workflow_id, input, out, config))
}

/// Execute the `run-workflow` subcommand.
/// Loads the config, resolves the workflow graph, runs the executor,
/// prints NodeStarted/NodeFinished to stderr and the terminal output to stdout
/// (or `--out <file>`).
async fn run_workflow_cmd(args: &[String]) -> Result<(), BoxError> {
    bridge_observ::init();
    let (workflow_id, input_path, out_path, config_path) = parse_run_workflow_args(args)?;

    // Load config.
    let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("run-workflow: cannot read config {:?}: {e}", config_path))?;
    let cfg = config::RegistryConfig::parse(&raw)
        .map_err(|e| format!("run-workflow: config parse error: {e}"))?;

    // Resolve prompt file base dir (same logic as `serve`).
    let base = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    let wf_map = cfg
        .load_workflows(&base)
        .map_err(|e| format!("run-workflow: workflow load error: {e}"))?;

    // Resolve workflow id.
    let wf_id = bridge_core::ids::WorkflowId::parse(workflow_id.clone())
        .map_err(|e| format!("run-workflow: invalid workflow id {workflow_id:?}: {e:?}"))?;
    let graph = wf_map
        .get(&wf_id)
        .cloned()
        .ok_or_else(|| format!("run-workflow: unknown workflow {workflow_id:?}"))?;

    // Build the registry + executor using the same SpawnFn the server uses.
    let snapshot = cfg
        .into_snapshot()
        .map_err(|e| format!("run-workflow: registry snapshot error: {e}"))?;
    let policy = Arc::new(bridge_policy::permission::AutoPolicy);
    let policy_for_spawn = Arc::clone(&policy) as Arc<dyn bridge_core::ports::PolicyEngine>;
    let spawn: bridge_registry::registry::SpawnFn = Arc::new(move |entry: Arc<AgentEntry>| {
        let policy = Arc::clone(&policy_for_spawn);
        Box::pin(async move {
            // The host child has no cwd (Supervised gets None); AcpConfig.cwd IS the ACP session cwd.
            // Resolution chain: session_cwd → cwd → "."; then absolutize relative values.
            let resolved = resolve_static_session_cwd(
                entry.session_cwd.as_deref(),
                entry.cwd.as_deref(),
            );
            let cwd = {
                let p = PathBuf::from(&resolved);
                if p.is_absolute() {
                    p
                } else {
                    std::env::current_dir()
                        .map_err(|e| BridgeError::ConfigInvalid {
                            reason: format!("cwd: {e}"),
                        })?
                        .join(p)
                }
            };
            let args: Vec<String> = entry.args.clone();
            let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
            use bridge_core::domain::AgentKind;
            match entry.kind {
                AgentKind::Acp => {
                    let cmd = entry.cmd.as_deref().ok_or(BridgeError::ConfigInvalid {
                        reason: format!("acp agent {} missing cmd", entry.id.as_str()),
                    })?;
                    let acp = bridge_acp::acp_backend::AcpConfig {
                        cwd,
                        model: entry.model.clone(),
                        mode: entry.mode.clone(),
                        auth_method: entry.auth_method.clone(),
                        ..bridge_acp::acp_backend::AcpConfig::default()
                    };
                    let be = bridge_acp::acp_backend::AcpBackend::spawn(cmd, &args_ref, acp)
                        .await?
                        .with_policy(policy);
                    Ok(Arc::new(be) as Arc<dyn bridge_core::ports::AgentBackend>)
                }
                AgentKind::Api => {
                    let base_url = entry.base_url.clone().ok_or(BridgeError::ConfigInvalid {
                        reason: format!("api agent {} missing base_url", entry.id.as_str()),
                    })?;
                    let mut api_cfg = bridge_api::ApiConfig::new(base_url);
                    api_cfg.model = entry.model.clone();
                    api_cfg.api_key_env = entry.api_key_env.clone();
                    let be = bridge_api::ApiBackend::new(api_cfg).with_policy(policy);
                    Ok(Arc::new(be) as Arc<dyn bridge_core::ports::AgentBackend>)
                }
            }
        })
    });
    let registry = Arc::new(
        bridge_registry::registry::Registry::new(snapshot, spawn)
            .map_err(|e| format!("run-workflow: registry init error: {e:?}"))?,
    );
    let executor = bridge_workflow::executor::WorkflowExecutor::new(
        Arc::clone(&registry) as Arc<dyn bridge_core::ports::AgentRegistry>
    );

    // Read input.
    let input = std::fs::read_to_string(&input_path)
        .map_err(|e| format!("run-workflow: cannot read input {:?}: {e}", input_path))?;

    // Unique run id.
    let run_id = format!(
        "cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );

    // Run the workflow.
    use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
    use futures::StreamExt;
    let mut stream = executor.run(
        graph,
        input,
        run_id,
        tokio_util::sync::CancellationToken::new(),
    );
    let mut output = String::new();
    let mut ok = true;
    while let Some(item) = stream.next().await {
        match item {
            Ok(WorkflowEvent::NodeStarted { node }) => {
                eprintln!("[workflow] node {} started", node.as_str());
            }
            Ok(WorkflowEvent::NodeFinished {
                node, ok: node_ok, ..
            }) => {
                eprintln!(
                    "[workflow] node {} {}",
                    node.as_str(),
                    if node_ok { "ok" } else { "failed" }
                );
            }
            Ok(WorkflowEvent::Terminal { outcome, output: o }) => {
                output = o;
                ok = matches!(outcome, WorkflowOutcome::Completed);
            }
            Err(e) => {
                eprintln!("[workflow] error: {e:?}");
                ok = false;
            }
        }
    }

    // Write output.
    if let Some(out) = out_path {
        std::fs::write(&out, &output)
            .map_err(|e| format!("run-workflow: cannot write output {:?}: {e}", out))?;
    } else {
        print!("{output}");
    }

    if ok {
        Ok(())
    } else {
        Err("run-workflow: workflow did not complete successfully".into())
    }
}

// ---------------------------------------------------------------------------
// A2A client helpers: submit + task get/list/cancel
// ---------------------------------------------------------------------------

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
}

async fn rpc_call(
    url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, BoxError> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    });
    let resp = reqwest::Client::new()
        .post(url)
        .header(a2a::SVC_PARAM_VERSION, a2a::VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            format!("cannot reach serve at {url} — is `a2a-bridge serve` running? ({e})")
        })?;
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("bad response: {e}"))?;
    Ok(v)
}

async fn submit_cmd(args: &[String]) -> Result<(), BoxError> {
    let skill = args.first().cloned().ok_or("submit: missing <skill>")?;
    let input_path = flag(args, "--input").ok_or("submit: --input <file> required")?;
    let url = flag(args, "--url").unwrap_or("http://127.0.0.1:8080");
    let text = std::fs::read_to_string(input_path)?;
    let params = serde_json::json!({
        "message": {
            "text": text,
            "metadata": { "a2a-bridge.skill": skill }
        }
    });
    let v = rpc_call(url, a2a::methods::SEND_MESSAGE, params).await?;
    if let Some(err) = v.get("error") {
        return Err(format!("submit failed: {err}").into());
    }
    let id = v["result"]["task"]["id"]
        .as_str()
        .ok_or("no task id in response")?;
    println!("{id}");
    Ok(())
}

async fn task_cmd(args: &[String]) -> Result<(), BoxError> {
    let sub = args
        .first()
        .map(|s| s.as_str())
        .ok_or("task: missing subcommand (get|list|cancel)")?;
    let url = flag(args, "--url").unwrap_or("http://127.0.0.1:8080");
    match sub {
        "get" => {
            let id = args.get(1).cloned().ok_or("task get: missing <id>")?;
            let v = rpc_call(
                url,
                a2a::methods::GET_TASK,
                serde_json::json!({ "taskId": id }),
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&v["result"]["task"])?);
        }
        "list" => {
            let limit: u64 = flag(args, "--limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(50);
            let v = rpc_call(
                url,
                a2a::methods::LIST_TASKS,
                serde_json::json!({ "limit": limit }),
            )
            .await?;
            for t in v["result"]["tasks"].as_array().cloned().unwrap_or_default() {
                println!(
                    "{}\t{}\t{}",
                    t["id"].as_str().unwrap_or("?"),
                    t["state"].as_str().unwrap_or("?"),
                    t["workflow"].as_str().unwrap_or("?")
                );
            }
        }
        "cancel" => {
            let id = args.get(1).cloned().ok_or("task cancel: missing <id>")?;
            let v = rpc_call(
                url,
                a2a::methods::CANCEL_TASK,
                serde_json::json!({ "taskId": id }),
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&v["result"]["task"])?);
        }
        other => return Err(format!("task: unknown subcommand {other:?}").into()),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    // Dispatch subcommands BEFORE the server path touches the filesystem.
    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.get(1).map(|s| s.as_str()) == Some("run-workflow") {
        return run_workflow_cmd(&raw_args[2..]).await;
    }
    if raw_args.get(1).map(|s| s.as_str()) == Some("submit") {
        return submit_cmd(&raw_args[2..]).await;
    }
    if raw_args.get(1).map(|s| s.as_str()) == Some("task") {
        return task_cmd(&raw_args[2..]).await;
    }

    // 1. Observability — install tracing subscriber (idempotent).
    bridge_observ::init();

    // 2. Configuration — ensure `a2a-bridge.toml` exists (materialise the built-in
    //    default if absent) so the FileConfigSource has a concrete file to load +
    //    a parent directory to watch.
    let config_path = PathBuf::from(CONFIG_PATH);
    if !config_path.exists() {
        std::fs::write(&config_path, DEFAULT_CONFIG)?;
    }

    // 3. Build the policy engine FIRST so the SAME engine drives both the inbound
    //    server's permission decisions AND each backend's REVERSE
    //    `session/request_permission` decisions (threaded via `with_policy`), so
    //    the system applies one consistent permission policy in both directions.
    let policy = Arc::new(AutoPolicy);

    // 4. SpawnFn — the registry's adapter factory. Lazily spawns a real AcpBackend
    //    per entry: it runs initialize → authenticate, owns the `Supervised` child
    //    for the backend's lifetime, and applies the configured mode/model after
    //    each `session/new`. `model`/`mode` here are the per-MINT FALLBACK; the
    //    per-session `configure_session` overrides them at dispatch (Task 6).
    let policy_for_spawn = Arc::clone(&policy) as Arc<dyn PolicyEngine>;
    let spawn: SpawnFn = Arc::new(move |entry: Arc<AgentEntry>| {
        let policy = Arc::clone(&policy_for_spawn);
        Box::pin(async move {
            // The host child has no cwd (Supervised gets None); AcpConfig.cwd IS the ACP session cwd.
            // Resolution chain: session_cwd → cwd → ".". A relative resolved value is joined
            // onto current_dir() to become absolute; an absolute one is used as-is (ACP §11A).
            let resolved = resolve_static_session_cwd(
                entry.session_cwd.as_deref(),
                entry.cwd.as_deref(),
            );
            let cwd = {
                let p = PathBuf::from(&resolved);
                if p.is_absolute() {
                    p
                } else {
                    std::env::current_dir()
                        .map_err(|e| BridgeError::ConfigInvalid {
                            reason: format!("cwd: {e}"),
                        })?
                        .join(p)
                }
            };
            let args: Vec<String> = entry.args.clone();
            let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
            use bridge_core::domain::AgentKind;
            match entry.kind {
                AgentKind::Acp => {
                    let cmd = entry.cmd.as_deref().ok_or(BridgeError::ConfigInvalid {
                        reason: format!("acp agent {} missing cmd", entry.id.as_str()),
                    })?;
                    let acp = AcpConfig {
                        cwd,
                        model: entry.model.clone(),
                        mode: entry.mode.clone(),
                        auth_method: entry.auth_method.clone(),
                        // handshake_timeout / cancel_grace: reuse the codebase defaults.
                        ..AcpConfig::default()
                    };
                    let be = AcpBackend::spawn(cmd, &args_ref, acp)
                        .await?
                        // Thread the system policy into the backend so its reverse-permission
                        // decisions match the inbound server's policy (Task 5/6).
                        .with_policy(policy);
                    Ok(Arc::new(be) as Arc<dyn AgentBackend>)
                }
                AgentKind::Api => {
                    let base_url = entry.base_url.clone().ok_or(BridgeError::ConfigInvalid {
                        reason: format!("api agent {} missing base_url", entry.id.as_str()),
                    })?;
                    let mut cfg = bridge_api::ApiConfig::new(base_url);
                    cfg.model = entry.model.clone();
                    cfg.api_key_env = entry.api_key_env.clone();
                    let be = bridge_api::ApiBackend::new(cfg).with_policy(policy);
                    Ok(Arc::new(be) as Arc<dyn AgentBackend>)
                }
            }
        })
    });

    // 5. Config source + registry. `load()` is the initial desired state; the
    //    delegation `[delegation]` env-expansion + `[server] addr` ride along on
    //    the RegistryConfig we re-read below for the non-registry fields.
    let source = FileConfigSource::new(config_path.clone());
    let snapshot = source.load().await?; // initial desired state
                                         // Fan-out source label (wire-observable in fan-out artifacts): the default
                                         // entry's `name` if set, else the default agent id, so a non-Kiro default
                                         // (e.g. codex) isn't mislabeled "kiro".
    let default_label = snapshot
        .entries
        .iter()
        .find(|e| e.id == snapshot.default)
        .and_then(|e| e.name.clone())
        .unwrap_or_else(|| snapshot.default.as_str().to_string());
    // Registry::new VALIDATES the snapshot → boot fails loud on bad config (spec §7).
    let registry = Arc::new(Registry::new(snapshot, spawn)?);

    // 6. Reconcile loop — consume `watch()` and `apply()` each new snapshot so
    //    on-disk edits hot-reload the live registry. The watch stream is held for
    //    the bridge's lifetime by the spawned task.
    {
        let reg = Arc::clone(&registry);
        let mut watch = source.watch();
        tokio::spawn(async move {
            use futures::StreamExt;
            while let Some(snap) = watch.next().await {
                if let Err(e) = reg.apply(snap).await {
                    tracing::error!(error = ?e, "registry reconcile failed");
                }
            }
        });
    }

    // 7. Build the remaining port Arc<dyn Trait> wrappers.
    let auth = Arc::new(AlwaysGrant);
    let store = Arc::new(SqliteStore::open_in_memory()?);

    // Read the non-registry config sections (server addr, delegation) directly:
    // the FileConfigSource snapshot only carries the registry. Re-parsing the same
    // file is cheap and keeps the [server]/[delegation] parsing (incl. env-expansion)
    // working on the RegistryConfig path.
    let raw = std::fs::read_to_string(&config_path)?;
    let cfg = RegistryConfig::parse(&raw)?;

    // 7a. Workflows (W1): load the [[workflows]] graphs (prompt files resolve
    //     relative to the config file's directory), build a WorkflowExecutor over
    //     the live registry, and wire both the route (skill->Workflow precedence)
    //     and the inbound server (the streaming workflow producer) with them.
    //     load_workflows fails loud (ConfigError) on a bad graph / missing prompt.
    let base = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let wf_map = cfg.load_workflows(base)?;
    let executor = Arc::new(bridge_workflow::executor::WorkflowExecutor::new(
        Arc::clone(&registry) as _,
    ));
    let route = Arc::new(SkillRoute::with_workflows(
        Arc::clone(&registry) as _,
        wf_map.keys().map(|k| k.as_str().to_string()).collect(),
    ));

    // Delegation port: real PeerDelegation when [delegation] is configured; StubDelegation otherwise.
    let delegation: Arc<dyn DelegationPort> = match &cfg.delegation {
        Some(d) => Arc::new(PeerDelegation::new(
            &d.peer_url,
            &d.auth,
            Duration::from_secs(d.timeout_secs),
        )),
        None => Arc::new(StubDelegation),
    };

    // W3b: durable task store. File-backed when [store] path is set (acquires the
    // single-serve lock via SqliteStore::open); else in-memory (ephemeral).
    // sweep_interrupted is REPLACED by resume_working_tasks for the file-backed path:
    // the resume routine decides per-task whether to re-run or interrupt based on the
    // workflow snapshot + attempt cap, rather than unconditionally sweeping all Working rows.
    let resume_cap = cfg
        .store
        .as_ref()
        .and_then(|s| s.resume_attempt_cap)
        .unwrap_or(3);
    let task_store: std::sync::Arc<dyn bridge_core::task_store::TaskStore> =
        match cfg.store.as_ref().map(|s| s.path.clone()) {
            Some(path) => {
                let s = std::sync::Arc::new(
                    SqliteStore::open(std::path::Path::new(&path))
                        .map_err(|e| format!("serve: cannot open task store {path:?}: {e:?}"))?,
                );
                s as std::sync::Arc<dyn bridge_core::task_store::TaskStore>
            }
            None => std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new()),
        };

    // 8. Construct the inbound server.
    //    InboundServer::new(registry, store, policy, route, auth, base_url, delegation, local_source_label)
    // The inbound server holds the agent registry (3b): first-message LOCAL dispatch
    // resolves the routed agent id, applies its effective config, and binds the task.
    let base_url = format!("http://{}", cfg.server.addr);
    let server = Arc::new(
        InboundServer::new(
            Arc::clone(&registry) as _,
            store,
            policy,
            route,
            auth,
            base_url,
            delegation,
            default_label.clone(),
        )
        .with_workflows(executor, wf_map.clone())
        .with_task_store(task_store)
        .with_allowed_cwd_root(cfg.allowed_cwd_root.clone()),
    );

    // 8b. Resume in-flight detached workflows from their checkpoints BEFORE accepting
    //     new requests. Boot order: open store → build server → resume → bind listener.
    //     For the in-memory/no-path branch the store is always empty so this is a no-op.
    bridge_a2a_inbound::server::resume_working_tasks(&server, resume_cap).await;

    // 8c. Build the axum router (consumes one Arc ref; hold a clone above for resume).
    let router = server.router();

    // 9. Bind and serve.
    let listener = tokio::net::TcpListener::bind(&cfg.server.addr).await?;
    tracing::info!(addr = %cfg.server.addr, agent = %default_label, "a2a-bridge listening");
    axum::serve(listener, router).await?;

    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn flag_parses_value_and_missing() {
        let a = vec![
            "--url".to_string(),
            "http://x".to_string(),
            "code-review".to_string(),
        ];
        assert_eq!(flag(&a, "--url"), Some("http://x"));
        assert_eq!(flag(&a, "--nope"), None);
    }

    #[test]
    fn resolve_static_session_cwd_chain() {
        assert_eq!(resolve_static_session_cwd(Some("/s"), Some("/c")), "/s"); // session_cwd wins
        assert_eq!(resolve_static_session_cwd(None, Some("/c")), "/c"); // falls to cwd
        assert_eq!(resolve_static_session_cwd(None, None), "."); // default
    }
}
