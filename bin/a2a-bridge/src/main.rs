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
//   a2a-bridge                                           — serve (./a2a-bridge.toml)
//   a2a-bridge serve [--config <path>]                   — serve an explicit config
//   a2a-bridge init [--dir <p>] [--agents ..] [--force]  — scaffold a config + prompts
//   a2a-bridge run-workflow <id> --input <file>
//             [--out <file>] [--config <path>]           — run a workflow offline
//   a2a-bridge submit <skill> --input <file> [--url <url>]
//                                                        — submit a detached task
//   a2a-bridge task get <id> [--url <url>]               — get task by id
//   a2a-bridge task list [--limit <n>] [--url <url>]     — list tasks
//   a2a-bridge task cancel <id> [--url <url>]            — cancel task by id
//   a2a-bridge task watch <id> [--from <seq>] [--url <url>]
//                                                        — stream a task's progress (SSE)

mod config;
mod implement;
mod review;
mod route;
mod tweak;
mod verify;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bridge_a2a_inbound::server::InboundServer;
use bridge_a2a_outbound::{PeerDelegation, StubDelegation};
use bridge_acp::acp_backend::AcpBackend;
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
const DEFAULT_CONFIG: &str = r#"# Single-agent zero-auth default. For codex + claude + the review workflows,
# run `a2a-bridge init` (or point `serve --config` at a multi-agent config).
default = "kiro"

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

/// Compose-or-raw: the `(runtime program, argv)` for spawning a `kind="acp"` agent. A `[sandbox]`
/// agent runs the runtime (docker) wrapping the agent cli; a raw agent runs `cmd`+`args` directly
/// (Slice A compat). BOTH `SpawnFn` closures (run-workflow + serve) call this, so the two paths can't
/// diverge. Unit-tested below; the Docker acceptance gate then proves it end-to-end.
fn acp_program_argv(
    entry: &AgentEntry,
    container_name: Option<&str>,
) -> Result<(String, Vec<String>), BridgeError> {
    let cmd = entry.cmd.as_deref().ok_or(BridgeError::ConfigInvalid {
        reason: format!("acp agent {} missing cmd", entry.id.as_str()),
    })?;
    Ok(match (&entry.sandbox, container_name) {
        // Named (`:ro` reaper) container when the caller supplied a name.
        (Some(sb), Some(name)) => {
            bridge_core::sandbox::compose_sandbox_named(sb, name, cmd, &entry.args)
        }
        (Some(sb), None) => bridge_core::sandbox::compose_sandbox(sb, cmd, &entry.args),
        (None, _) => (cmd.to_string(), entry.args.clone()),
    })
}

/// Build `(program, argv, AcpConfig)` for a `kind=acp` agent, attaching the `:ro` container reaper when the
/// sandbox is `access=ro` (owner-scoped name `a2a-ro-<owner>-<nonce>` + a `docker rm -f` reap_fn). Shared by
/// BOTH spawn factories (`make_spawn_fn` + `serve`) so the `:ro` naming/reaping can't diverge.
fn acp_spawn_inputs(
    entry: &AgentEntry,
    cwd: PathBuf,
    owner_config_path: &std::path::Path,
) -> Result<(String, Vec<String>, bridge_acp::acp_backend::AcpConfig), BridgeError> {
    use bridge_core::domain::MountAccess;
    let ro_name = entry
        .sandbox
        .as_ref()
        .filter(|sb| matches!(sb.access, MountAccess::Ro))
        .map(|sb| {
            let owner = container_owner(owner_config_path, &sb.mount, entry.id.as_str());
            bridge_core::sandbox::ro_container_name(&owner, &implement::nonce(8))
        });
    let (program, argv) = acp_program_argv(entry, ro_name.as_deref())?;
    let container = ro_name.map(|name| bridge_acp::acp_backend::ContainerReap {
        runtime: entry
            .sandbox
            .as_ref()
            .map(|sb| sb.runtime().to_string())
            .unwrap_or_else(|| "docker".to_string()),
        name,
        reap_fn: bridge_core::reaper::production_reap_fn(),
    });
    let acp = bridge_acp::acp_backend::AcpConfig {
        cwd,
        model: entry.model.clone(),
        mode: entry.mode.clone(),
        auth_method: entry.auth_method.clone(),
        container,
        ..bridge_acp::acp_backend::AcpConfig::default()
    };
    Ok((program, argv, acp))
}

/// Production [`bridge_container::ContainerSpawn`]: spawn a real `AcpBackend` inside the composed
/// container and apply the system policy (mirrors the `Acp` arm's `.with_policy`). The policy lives
/// HERE, not on `ContainerRwBackend` (which only forwards to the inner).
struct AcpContainerSpawn {
    policy: Arc<dyn bridge_core::ports::PolicyEngine>,
}
#[async_trait::async_trait]
impl bridge_container::ContainerSpawn for AcpContainerSpawn {
    async fn spawn(
        &self,
        program: &str,
        argv: &[String],
        cfg: bridge_acp::acp_backend::AcpConfig,
    ) -> Result<Arc<dyn bridge_core::ports::AgentBackend>, BridgeError> {
        let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
        let be = bridge_acp::acp_backend::AcpBackend::spawn(program, &argv_ref, cfg)
            .await?
            .with_policy(Arc::clone(&self.policy));
        Ok(Arc::new(be) as Arc<dyn bridge_core::ports::AgentBackend>)
    }
}

/// Stable per-instance owner token for `ContainerRw` container names: hash of the canonical config
/// path + the mount anchor + the agent id. STABLE across restarts (a restarted process reaps its OWN
/// crash orphans) and UNIQUE per agent (two `container_rw` agents can't collide / cross-reap).
fn container_owner(config_path: &std::path::Path, mount: &str, agent_id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    config_path.hash(&mut h);
    mount.hash(&mut h);
    agent_id.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The `(runtime, owner)` sweep targets for THIS bridge instance's `:ro` agents. Reads the SNAPSHOT
/// (normalized mount + typed kind/access) so the owner matches the spawn-time owner; `config_path` must be
/// the SAME (canonical) value `make_spawn_fn` is given. Scoped per (config, mount, agent) owner so a
/// CONCURRENT bridge's containers are untouched.
fn ro_sweep_targets(
    snapshot: &bridge_core::domain::RegistrySnapshot,
    config_path: &std::path::Path,
) -> Vec<(String, String)> {
    use bridge_core::domain::{AgentKind, MountAccess};
    let mut targets = Vec::new();
    for entry in &snapshot.entries {
        let Some(sb) = entry.sandbox.as_ref() else {
            continue;
        };
        if entry.kind != AgentKind::Acp || !matches!(sb.access, MountAccess::Ro) {
            continue;
        }
        let owner = container_owner(config_path, &sb.mount, entry.id.as_str());
        targets.push((sb.runtime().to_string(), owner));
    }
    targets
}

/// SYNCHRONOUS owner-scoped reap of `a2a-ro-<owner>-` containers (best-effort, blocking). Used for both
/// the boot-sweep (crash recovery) and the one-shot END-sweep — the latter must be synchronous because the
/// per-backend Drop reaper detaches its `docker rm -f`, which races process exit on a one-shot command
/// (run-workflow/implement) where the runtime dies before the detached task runs (live-gate finding).
fn ro_sweep(targets: &[(String, String)]) {
    for (runtime, owner) in targets {
        let (prog, argv) = bridge_core::sandbox::ro_sweep_filter_argv(runtime, owner);
        if let Ok(out) = std::process::Command::new(&prog).args(&argv).output() {
            for id in String::from_utf8_lossy(&out.stdout).split_whitespace() {
                let _ = std::process::Command::new(runtime)
                    .args(["rm", "-f", id])
                    .output();
            }
        }
    }
}

/// RAII end-sweep for one-shot commands: on drop (any return path) it synchronously reaps this run's `:ro`
/// containers, so a clean `run-workflow`/`implement` exit doesn't leak (the detached Drop reaper races the
/// process exit). Declared early so it drops AFTER the registry/backends (whose detached reaps fire first).
struct RoSweepGuard(Vec<(String, String)>);
impl Drop for RoSweepGuard {
    fn drop(&mut self) {
        ro_sweep(&self.0);
    }
}

/// The production `SpawnFn` (Acp compose-or-raw / Api / ContainerRw arms) — shared by run-workflow and the
/// `implement` subcommand so their registry builds can't drift. `owner_config_path` seeds the ContainerRw
/// owner token.
fn make_spawn_fn(
    policy_for_spawn: Arc<dyn bridge_core::ports::PolicyEngine>,
    owner_config_path: PathBuf,
) -> bridge_registry::registry::SpawnFn {
    Arc::new(move |entry: Arc<AgentEntry>| {
        let policy = Arc::clone(&policy_for_spawn);
        let owner_config_path = owner_config_path.clone();
        Box::pin(async move {
            // The host child has no cwd (Supervised gets None); AcpConfig.cwd IS the ACP session cwd.
            // Resolution chain: session_cwd → cwd → "."; then absolutize relative values.
            let resolved =
                resolve_static_session_cwd(entry.session_cwd.as_deref(), entry.cwd.as_deref());
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
            use bridge_core::domain::AgentKind;
            match entry.kind {
                AgentKind::Acp => {
                    // Compose-or-raw + the `:ro` reaper via the shared helper (both factories agree).
                    let (program, argv, acp) = acp_spawn_inputs(&entry, cwd, &owner_config_path)?;
                    let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
                    let be = bridge_acp::acp_backend::AcpBackend::spawn(&program, &argv_ref, acp)
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
                AgentKind::ContainerRw => {
                    let sb = entry.sandbox.clone().ok_or(BridgeError::ConfigInvalid {
                        reason: format!(
                            "container_rw agent {} requires sandbox",
                            entry.id.as_str()
                        ),
                    })?;
                    let cmd = entry.cmd.clone().ok_or(BridgeError::ConfigInvalid {
                        reason: format!("container_rw agent {} requires cmd", entry.id.as_str()),
                    })?;
                    let owner = container_owner(&owner_config_path, &sb.mount, entry.id.as_str());
                    let ccfg = bridge_container::ContainerRwConfig {
                        sandbox: sb,
                        cmd,
                        args: entry.args.clone(),
                        model: entry.model.clone(),
                        mode: entry.mode.clone(),
                        auth_method: entry.auth_method.clone(),
                        handshake_timeout: bridge_acp::acp_backend::AcpConfig::default()
                            .handshake_timeout,
                        cancel_grace: bridge_acp::acp_backend::AcpConfig::default().cancel_grace,
                    };
                    let cspawn: Arc<dyn bridge_container::ContainerSpawn> =
                        Arc::new(AcpContainerSpawn {
                            policy: Arc::clone(&policy),
                        });
                    let be = bridge_container::ContainerRwBackend::new(ccfg, cspawn, owner).await?;
                    Ok(Arc::new(be) as Arc<dyn bridge_core::ports::AgentBackend>)
                }
            }
        })
    })
}

/// Parse `a2a-bridge run-workflow <id> --input <file> [--out <file>] [--config <path>]`
/// from a raw args iterator (skipping the binary name at position 0 and the
/// subcommand name at position 1).
#[allow(clippy::type_complexity)]
fn parse_run_workflow_args(
    args: &[String],
) -> Result<(String, PathBuf, Option<PathBuf>, PathBuf, Option<String>), BoxError> {
    let mut iter = args.iter().peekable();
    // Positional: workflow id
    let workflow_id = iter
        .next()
        .cloned()
        .ok_or("run-workflow: missing <workflow-id>")?;
    let mut input: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut config: Option<PathBuf> = None;
    // The per-request ACP session cwd (the writable target for a container_rw agent, or the repo a
    // reader works in). Without it, run-workflow agents run in the LAUNCH cwd, not the target repo.
    let mut session_cwd: Option<String> = None;
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
            "--session-cwd" => {
                session_cwd = Some(
                    iter.next()
                        .ok_or("run-workflow: --session-cwd requires a value")?
                        .clone(),
                );
            }
            other => return Err(format!("run-workflow: unknown flag {other:?}").into()),
        }
    }
    let input = input.ok_or("run-workflow: --input <file> is required")?;
    let config = config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH));
    Ok((workflow_id, input, out, config, session_cwd))
}

// ---------------------------------------------------------------------------
// `implement` subcommand (Slice B2b-1)
// ---------------------------------------------------------------------------

struct ImplementArgs {
    task: String,
    repo: PathBuf,
    base_ref: Option<String>,
    config: PathBuf,
    workflow: String,
}

const IMPLEMENT_USAGE: &str =
    "usage: a2a-bridge implement <task> --repo <path> [--base-ref <ref>] [--config <path>] [--workflow <id>]";

fn parse_implement_args(args: &[String]) -> Result<ImplementArgs, BoxError> {
    let mut iter = args.iter();
    let task = iter
        .next()
        .cloned()
        .ok_or_else(|| format!("implement: missing <task>\n{IMPLEMENT_USAGE}"))?;
    if task.starts_with("--") {
        return Err(
            format!("implement: missing <task> (got flag {task:?})\n{IMPLEMENT_USAGE}").into(),
        );
    }
    let (mut repo, mut base_ref, mut config, mut workflow) = (None, None, None, None);
    while let Some(f) = iter.next() {
        match f.as_str() {
            "--repo" => {
                repo = Some(PathBuf::from(
                    iter.next().ok_or("implement: --repo needs a value")?,
                ))
            }
            "--base-ref" => {
                base_ref = Some(
                    iter.next()
                        .ok_or("implement: --base-ref needs a value")?
                        .clone(),
                )
            }
            "--config" => {
                config = Some(PathBuf::from(
                    iter.next().ok_or("implement: --config needs a value")?,
                ))
            }
            "--workflow" => {
                workflow = Some(
                    iter.next()
                        .ok_or("implement: --workflow needs a value")?
                        .clone(),
                )
            }
            other => {
                return Err(format!("implement: unknown flag {other:?}\n{IMPLEMENT_USAGE}").into())
            }
        }
    }
    Ok(ImplementArgs {
        task,
        repo: repo
            .ok_or_else(|| format!("implement: --repo <path> is required\n{IMPLEMENT_USAGE}"))?,
        base_ref,
        config: config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH)),
        workflow: workflow.unwrap_or_else(|| "implement-edit".into()),
    })
}

/// B2b-3a: drain a review workflow stream → (completed, synth_output, reviewers_failed). Collects events +
/// delegates the reduction to the PURE `review::reduce`. `run_with_context` already returns a boxed
/// `WorkflowStream`, so take it directly (no `Box::pin`). Keeps polling to the end so the executor runs its
/// cancel cleanup (backend.cancel/forget_session) even after a timeout cancel.
async fn drain_review(
    mut stream: bridge_workflow::executor::WorkflowStream,
) -> (bool, String, usize) {
    use futures::StreamExt;
    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(ev) => events.push(ev),
            Err(e) => eprintln!("[implement] review: stream error: {e:?}"),
        }
    }
    review::reduce(&events)
}

/// `a2a-bridge implement <task> --repo <path>` — clone a quarantine, run the 1-node `implement-edit`
/// workflow on the ContainerRw `impl` agent (session_cwd = the clone), then the deterministic commit
/// state machine + the operator hand-off. The agent owns staging + the message; the bridge owns the commit.
async fn implement_cmd(args: &[String]) -> Result<(), BoxError> {
    bridge_observ::init();
    let a = parse_implement_args(args)?;

    // 1. config + canonical allowed_cwd_root (the ContainerRw mount anchor).
    let raw = std::fs::read_to_string(&a.config)
        .map_err(|e| format!("implement: read config {:?}: {e}", a.config))?;
    let cfg =
        config::RegistryConfig::parse(&raw).map_err(|e| format!("implement: config parse: {e}"))?;
    let root = cfg
        .allowed_cwd_root
        .clone()
        .ok_or("implement: config needs allowed_cwd_root (the ContainerRw mount anchor)")?;
    let root = std::fs::canonicalize(&root)
        .map_err(|e| format!("implement: allowed_cwd_root {root:?}: {e}"))?;

    // R6: probe the EXISTING root for an enclosing worktree BEFORE creating .a2a-implement.
    implement::assert_dest_outside_worktree(&root)?;
    let impl_dir = root.join(".a2a-implement");
    std::fs::create_dir_all(&impl_dir)
        .map_err(|e| format!("implement: mkdir {impl_dir:?}: {e}"))?;

    // 2. task-id (collision-retry) + clone dest.
    let (task_id, clone) = {
        let mut chosen = None;
        for _ in 0..8 {
            let id = implement::task_id(std::process::id(), &implement::nonce(8));
            let dir = impl_dir.join(&id);
            if !dir.exists() {
                chosen = Some((id, dir));
                break;
            }
        }
        chosen.ok_or("implement: could not find a free task-id")?
    };

    // R9: resolve the base ref (or the source HEAD) to a SHA for determinism.
    let refname = a.base_ref.clone().unwrap_or_else(|| "HEAD".into());
    let rp = implement::run_git(Some(&a.repo), &["rev-parse", &refname])
        .map_err(|e| format!("implement: rev-parse {refname:?} in {:?}: {e}", a.repo))?;
    if !rp.status.success() {
        return Err(format!(
            "implement: base-ref {refname:?}: {}",
            String::from_utf8_lossy(&rp.stderr).trim()
        )
        .into());
    }
    let base_sha = String::from_utf8_lossy(&rp.stdout).trim().to_string();

    // 3. clone (committed-only) + checkout the base SHA + the task branch.
    implement::do_clone(&a.repo.to_string_lossy(), &clone.to_string_lossy())?;
    let co = implement::run_git(Some(&clone), &["checkout", "-q", &base_sha])
        .map_err(|e| format!("implement: checkout {base_sha}: {e}"))?;
    if !co.status.success() {
        return Err(format!(
            "implement: checkout {base_sha}: {}",
            String::from_utf8_lossy(&co.stderr).trim()
        )
        .into());
    }
    let branch = implement::branch_for(&task_id);
    implement::do_checkout_branch(&clone, &branch)?;
    let pre = implement::head_sha(&clone)?;
    // Precompute the clone's SessionCwd ONCE (pre-commit, fallible here) — reused by the implement-edit
    // ctx, verify, and review so NO `SessionCwd::parse` runs after the commit (the hand-off must always
    // print). B2b-3a: removes the latent post-commit verify parse too.
    let clone_cwd = bridge_core::SessionCwd::parse(&clone.to_string_lossy())?;
    let _ = std::fs::remove_file(clone.join(".git").join("A2A_COMMIT_MSG")); // R13: strip before

    // 4. run the 1-node implement-edit workflow with session_cwd = the clone.
    let base = a
        .config
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    let wf_map = cfg
        .load_workflows(&base)
        .map_err(|e| format!("implement: workflow load: {e}"))?;
    let wf_id = bridge_core::ids::WorkflowId::parse(a.workflow.clone())
        .map_err(|e| format!("implement: workflow id {:?}: {e:?}", a.workflow))?;
    let graph = wf_map
        .get(&wf_id)
        .cloned()
        .ok_or_else(|| format!("implement: unknown workflow {:?}", a.workflow))?;
    // B2b-2: parse [verify] NOW, before into_snapshot moves cfg. Owned + Clone, survives the move.
    let verify_cfg = cfg.verify.as_ref().map(|t| t.to_config());
    let review_cfg = cfg.review.as_ref().map(|t| t.to_config()); // B2b-3a: parsed pre-commit (beside verify)
    let snapshot = cfg
        .into_snapshot()
        .map_err(|e| format!("implement: snapshot: {e}"))?;
    // Canonical config path: the owner token must match between the sweeps and the spawn factory.
    let owner_config_path = std::fs::canonicalize(&a.config).unwrap_or_else(|_| a.config.clone());
    let ro_targets = ro_sweep_targets(&snapshot, &owner_config_path);
    ro_sweep(&ro_targets); // boot-sweep (crash recovery)
    let _ro_guard = RoSweepGuard(ro_targets); // synchronous END-sweep on any return (one-shot reliability)
    let policy = Arc::new(AutoPolicy);
    let policy_for_spawn = Arc::clone(&policy) as Arc<dyn PolicyEngine>;
    let spawn = make_spawn_fn(policy_for_spawn, owner_config_path);
    let registry = Arc::new(
        bridge_registry::registry::Registry::new(snapshot, spawn)
            .map_err(|e| format!("implement: registry: {e:?}"))?,
    );
    let executor = bridge_workflow::executor::WorkflowExecutor::new(
        Arc::clone(&registry) as Arc<dyn bridge_core::ports::AgentRegistry>
    );
    let run_id = format!("impl-{task_id}");
    let ctx = bridge_workflow::executor::WorkflowRunContext {
        session_cwd: Some(clone_cwd.clone()),
    };
    use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
    use futures::StreamExt;
    let mut stream = executor.run_with_context(
        graph,
        a.task.clone(),
        run_id,
        tokio_util::sync::CancellationToken::new(),
        ctx,
    );
    let mut completed = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(WorkflowEvent::NodeStarted { node }) => {
                eprintln!("[implement] node {} started", node.as_str())
            }
            Ok(WorkflowEvent::NodeFinished { node, ok, .. }) => {
                eprintln!(
                    "[implement] node {} {}",
                    node.as_str(),
                    if ok { "ok" } else { "failed" }
                )
            }
            Ok(WorkflowEvent::Terminal { outcome, .. }) => {
                completed = matches!(outcome, WorkflowOutcome::Completed)
            }
            Err(e) => eprintln!("[implement] error: {e:?}"),
        }
    }
    drop(stream); // end the run; the per-turn ContainerRw container is reaped (detached).

    // 5. the pure soft-gate decision, then execute its Action.
    let guard = implement::head_guard(&clone, &branch, &pre);
    let stage =
        implement::stage_state(&clone).map_err(|e| format!("implement: stage check: {e}"))?;
    let msg = implement::commit_message(implement::read_commit_msg_file(&clone), &a.task);
    if msg.1 {
        eprintln!("[implement] no .git/A2A_COMMIT_MSG — using task-derived message");
    }
    match implement::decide(completed, guard, stage, msg) {
        implement::Action::Abort(reason) => {
            eprintln!(
                "[implement] {reason} — NO commit; clone left at {}",
                clone.display()
            );
            Err(format!("implement: {reason}").into())
        }
        implement::Action::NoCommitClean => {
            println!(
                "implement: made no changes; clone left at {}",
                clone.display()
            );
            Ok(())
        }
        implement::Action::NoCommitDirty => {
            eprintln!(
                "[implement] agent edited but staged NOTHING — NOT committing (agent owns staging). \
                 Clone left at {} for inspection.",
                clone.display()
            );
            Ok(())
        }
        implement::Action::Commit(message) => {
            let sha = implement::host_commit(&clone, &message)?;
            let _ = std::fs::remove_file(clone.join(".git").join("A2A_COMMIT_MSG")); // R13: strip after
                                                                                     // Best-effort (NO `?`): the post-commit tail must always reach the hand-off (B2b-3a invariant).
            if !matches!(
                implement::stage_state(&clone).unwrap_or(implement::StageState::Clean),
                implement::StageState::Clean
            ) {
                eprintln!("[implement] note: the clone still has uncommitted changes the agent left unstaged.");
            }
            let subject = message.lines().next().unwrap_or("").to_string();
            let mut handoff = implement::handoff_text(
                &clone.to_string_lossy(),
                &branch,
                &sha,
                &subject,
                &a.repo.to_string_lossy(),
            );

            // B2b-2: deterministic build+test verify on the committed clone (reported, not gating).
            // `verify_cfg` was captured before into_snapshot. The pure `outcome_suffix` (verify.rs) does
            // the riskiest classification; this arm only resolves the outcome (impure) + dumps stderr.
            let outcome = match verify_cfg {
                Some(Ok(vcfg)) => {
                    // Canonicalize a.repo for the cache key so two spellings don't split the warm cache.
                    let repo_canon =
                        std::fs::canonicalize(&a.repo).unwrap_or_else(|_| a.repo.clone());
                    let cache_vol =
                        verify::cache_volume_name(&vcfg.cache, &repo_canon.to_string_lossy());
                    eprintln!(
                        "[implement] verify: running {} command(s) in {}",
                        vcfg.commands.len(),
                        vcfg.image
                    );
                    let verdict = verify::run_verify(
                        &vcfg,
                        &clone_cwd,
                        &cache_vol,
                        &verify::docker_runner,
                        16 * 1024,
                    );
                    for r in &verdict.results {
                        if !r.ok {
                            eprintln!("[implement] verify: {} failed:\n{}", r.name, r.output);
                        }
                    }
                    verify::VerifyOutcome::Ran(verdict)
                }
                Some(Err(e)) => {
                    eprintln!("[implement] verify: config error: {e:?} — skipping verify");
                    verify::VerifyOutcome::ConfigError
                }
                None => verify::VerifyOutcome::NotConfigured,
            };
            handoff.push('\n');
            handoff.push_str(&verify::outcome_suffix(&outcome));

            // B2b-3a: advisory review of the committed diff (bounded; never blocks the hand-off — NO `?`).
            let review_outcome = match review_cfg {
                None => review::ReviewOutcome::NotConfigured,
                Some(Err(e)) => {
                    eprintln!("[implement] review: config error: {e:?}");
                    review::ReviewOutcome::ConfigError
                }
                Some(Ok(rcfg)) => match wf_map.get(&rcfg.workflow).cloned() {
                    None => review::ReviewOutcome::NotLoaded,
                    Some(graph) => {
                        let input = review::build_review_input(&a.task, &base_sha, &sha);
                        let ctx = bridge_workflow::executor::WorkflowRunContext {
                            session_cwd: Some(clone_cwd.clone()),
                        };
                        let token = tokio_util::sync::CancellationToken::new();
                        let stream = executor.run_with_context(
                            graph,
                            input,
                            format!("impl-review-{task_id}"),
                            token.clone(),
                            ctx,
                        );
                        eprintln!("[implement] review: running implement-review");
                        // Timeout = cancel-then-KEEP-DRAINING (don't drop the stream — the executor must
                        // keep being polled to run backend.cancel/forget_session on cancel).
                        let mut drain = std::pin::pin!(drain_review(stream));
                        let (completed, synth, reviewers_failed) = tokio::select! {
                            r = &mut drain => r,
                            _ = tokio::time::sleep(rcfg.timeout) => {
                                eprintln!("[implement] review: timed out after {:?}", rcfg.timeout);
                                token.cancel();
                                (&mut drain).await
                            }
                        };
                        if completed {
                            // Parse the FULL synth (truncation could drop a body VERDICT → flip it).
                            let (verdict, summary) = review::parse_verdict(&synth);
                            if !matches!(verdict, review::Verdict::Approve) {
                                eprintln!(
                                    "[implement] review {verdict:?}:\n{}",
                                    verify::truncate_output(&synth, rcfg.max_output_bytes)
                                );
                            }
                            review::ReviewOutcome::Ran {
                                verdict,
                                summary,
                                reviewers_failed,
                            }
                        } else {
                            review::ReviewOutcome::Incomplete
                        }
                    }
                },
            };
            handoff.push('\n');
            handoff.push_str(&review::outcome_suffix(&review_outcome));

            println!("{handoff}");
            Ok(())
        }
    }
}

/// Execute the `run-workflow` subcommand.
/// Loads the config, resolves the workflow graph, runs the executor,
/// prints NodeStarted/NodeFinished to stderr and the terminal output to stdout
/// (or `--out <file>`).
async fn run_workflow_cmd(args: &[String]) -> Result<(), BoxError> {
    bridge_observ::init();
    let (workflow_id, input_path, out_path, config_path, session_cwd) =
        parse_run_workflow_args(args)?;

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
    // Canonical config path: the owner token must match between the sweeps and the spawn factory.
    let owner_config_path =
        std::fs::canonicalize(&config_path).unwrap_or_else(|_| config_path.clone());
    let ro_targets = ro_sweep_targets(&snapshot, &owner_config_path);
    ro_sweep(&ro_targets); // boot-sweep (crash recovery)
    let _ro_guard = RoSweepGuard(ro_targets); // synchronous END-sweep on any return (one-shot reliability)
    let policy = Arc::new(bridge_policy::permission::AutoPolicy);
    let policy_for_spawn = Arc::clone(&policy) as Arc<dyn bridge_core::ports::PolicyEngine>;
    let spawn = make_spawn_fn(policy_for_spawn, owner_config_path);
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

    // Per-request session cwd: thread it into the context so EVERY node's agent works in the target
    // dir (a container_rw :rw target, or the repo a reader reads) — not the launch cwd.
    let ctx = match session_cwd {
        Some(dir) => {
            let cwd = bridge_core::SessionCwd::parse(&dir)
                .map_err(|e| format!("run-workflow: invalid --session-cwd {dir:?}: {e:?}"))?;
            bridge_workflow::executor::WorkflowRunContext {
                session_cwd: Some(cwd),
            }
        }
        None => bridge_workflow::executor::WorkflowRunContext::default(),
    };

    // Run the workflow.
    use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome};
    use futures::StreamExt;
    let mut stream = executor.run_with_context(
        graph,
        input,
        run_id,
        tokio_util::sync::CancellationToken::new(),
        ctx,
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
        .ok_or("task: missing subcommand (get|list|cancel|watch)")?;
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
        "watch" => {
            let id = args.get(1).cloned().ok_or("task watch: missing <id>")?;
            let from: Option<i64> = flag(args, "--from").and_then(|s| s.parse().ok());
            task_watch_cmd(url, &id, from).await?;
        }
        other => return Err(format!("task: unknown subcommand {other:?}").into()),
    }
    Ok(())
}

/// Execute `task watch <id> [--from <seq>]`: POSTs a SubscribeToTask JSON-RPC
/// request and streams the SSE response, printing each `data:` line to stdout.
/// Tracks the last `id:` value seen and prints a resume hint on stream close.
async fn task_watch_cmd(url: &str, id: &str, from: Option<i64>) -> Result<(), BoxError> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "SubscribeToTask",
        "params": { "id": id }
    });

    let mut req = reqwest::Client::new()
        .post(url)
        .header(a2a::SVC_PARAM_VERSION, a2a::VERSION)
        .json(&body);
    if let Some(cursor) = from {
        req = req.header("Last-Event-ID", cursor.to_string());
    }

    let resp = req.send().await.map_err(|e| {
        format!("cannot reach serve at {url} — is `a2a-bridge serve` running? ({e})")
    })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        eprintln!("error: HTTP {status}\n{text}");
        return Err(format!("task watch: server returned {status}").into());
    }

    // Stream the SSE response: accumulate bytes, split on newlines, handle
    // `id:` and `data:` lines; ignore blank lines and comments.
    use futures::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut last_id: Option<String> = None;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("task watch: stream error: {e}"))?;
        let text = String::from_utf8_lossy(&chunk);
        buf.push_str(&text);

        // Process all complete lines (ending with '\n') in the buffer.
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim_end_matches('\r').to_string();
            buf.drain(..=pos);

            if let Some(data) = line.strip_prefix("data:") {
                let payload = data.trim_start();
                println!("{payload}");
            } else if let Some(seq) = line.strip_prefix("id:") {
                last_id = Some(seq.trim_start().to_string());
            }
            // Blank lines and comment lines (`:`) are silently ignored.
        }
    }

    // Print resume hint if we received at least one id: line.
    if let Some(seq) = last_id {
        eprintln!("# stream closed; resume with --from {seq}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// `serve --config` flag + `init` scaffold
// ---------------------------------------------------------------------------

/// Parse the `serve` subcommand's flags. Only `--config <path>` is accepted;
/// any other token errors (so a typo'd flag is not silently ignored).
fn serve_config_flag(args: &[String]) -> Result<Option<PathBuf>, BoxError> {
    let mut iter = args.iter();
    let mut config: Option<PathBuf> = None;
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--config" => {
                config = Some(PathBuf::from(
                    iter.next().ok_or("serve: --config requires a <path>")?,
                ));
            }
            other => {
                return Err(format!("serve: unknown flag {other:?} (only --config <path>)").into());
            }
        }
    }
    Ok(config)
}

/// The review prompt files embedded in the binary so `init` is self-contained
/// (no bridge repo needed at runtime). `(relative-output-path, contents)`.
const INIT_PROMPTS: &[(&str, &str)] = &[
    (
        "prompts/review-implement.md",
        include_str!("../../../prompts/review-implement.md"),
    ),
    (
        "prompts/review-implement-synth.md",
        include_str!("../../../prompts/review-implement-synth.md"),
    ),
    (
        "prompts/review-correctness.md",
        include_str!("../../../prompts/review-correctness.md"),
    ),
    (
        "prompts/review-architecture.md",
        include_str!("../../../prompts/review-architecture.md"),
    ),
    (
        "prompts/review-synth.md",
        include_str!("../../../prompts/review-synth.md"),
    ),
    (
        "prompts/spec-review-rigor.md",
        include_str!("../../../prompts/spec-review-rigor.md"),
    ),
    (
        "prompts/spec-review-soundness.md",
        include_str!("../../../prompts/spec-review-soundness.md"),
    ),
    (
        "prompts/spec-review-synth.md",
        include_str!("../../../prompts/spec-review-synth.md"),
    ),
    (
        "prompts/plan-review-exec.md",
        include_str!("../../../prompts/plan-review-exec.md"),
    ),
    (
        "prompts/plan-review-coverage.md",
        include_str!("../../../prompts/plan-review-coverage.md"),
    ),
    (
        "prompts/plan-review-synth.md",
        include_str!("../../../prompts/plan-review-synth.md"),
    ),
    (
        "prompts/design-executability.md",
        include_str!("../../../prompts/design-executability.md"),
    ),
    (
        "prompts/design-structure.md",
        include_str!("../../../prompts/design-structure.md"),
    ),
    (
        "prompts/design-synth.md",
        include_str!("../../../prompts/design-synth.md"),
    ),
];

const INIT_README: &str = include_str!("init-readme-template.md");

/// The four agents `init` knows how to scaffold. `acp_cmd` is `Some` for process
/// (ACP) agents — those go into `allowed_cmds`; `None` for the non-process `api` agent.
fn known_init_agents() -> [(&'static str, Option<&'static str>); 4] {
    [
        ("kiro", Some("kiro-cli")),
        ("codex", Some("codex-acp")),
        ("claude", Some("claude-agent-acp")),
        ("api", None),
    ]
}

/// A TOML `[[agents]]` fragment for one known agent. `model`/`effort` are shown;
/// `mode` is intentionally omitted (a bad `mode` HARD-fails session/set_mode).
fn agent_fragment(name: &str) -> &'static str {
    match name {
        "kiro" => "\n# kiro: zero-auth local default (kiro-cli acp).\n[[agents]]\nid   = \"kiro\"\ncmd  = \"kiro-cli\"\nargs = [\"acp\"]\nmodel = \"auto\"\n",
        "codex" => "\n# codex: gpt-5.5 with reasoning_effort (effort is codex-only).\n[[agents]]\nid    = \"codex\"\ncmd   = \"codex-acp\"\nmodel = \"gpt-5.5\"\neffort = \"high\"\n",
        "claude" => "\n# claude: subscription. NOTE: claude's model is NOT observable through the\n# bridge (claude-agent-acp uses the subscription default; set_model is best-effort).\n[[agents]]\nid    = \"claude\"\ncmd   = \"claude-agent-acp\"\nmodel = \"sonnet\"\n",
        "api" => "\n# api: OpenAI-compatible non-process backend. `api_key_env` is the NAME of an\n# env var holding the token (never the secret itself). Effort is not applied for api.\n[[agents]]\nid          = \"api\"\nkind        = \"api\"\nbase_url    = \"https://api.openai.com/v1\"\napi_key_env = \"OPENAI_API_KEY\"\nmodel       = \"gpt-4o-mini\"\n",
        _ => "",
    }
}

/// The three review workflows (relative `prompts/` paths for `init` output).
/// All reference both `codex` and `claude`, so they're only emitted when both
/// are selected (else `load_workflows` would fail on a missing agent at boot).
const INIT_WORKFLOWS: &str = r#"
# ── Review workflows (two independent lenses + a synthesis) ──
[[workflows]]
id = "code-review"
[[workflows.nodes]]
id = "correctness"
agent = "codex"
prompt_file = "prompts/review-correctness.md"
inputs = []
[[workflows.nodes]]
id = "architecture"
agent = "claude"
prompt_file = "prompts/review-architecture.md"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt_file = "prompts/review-synth.md"
inputs = ["correctness", "architecture"]

# ── implement-review (B2b-3a): two folded reviewers of the committed diff → synth verdict ──
[[workflows]]
id = "implement-review"
[[workflows.nodes]]
id = "reviewer_codex"
agent = "codex"
prompt_file = "prompts/review-implement.md"
inputs = []
[[workflows.nodes]]
id = "reviewer_claude"
agent = "claude"
prompt_file = "prompts/review-implement.md"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt_file = "prompts/review-implement-synth.md"
inputs = ["reviewer_codex", "reviewer_claude"]

[[workflows]]
id = "spec-review"
[[workflows.nodes]]
id = "rigor"
agent = "codex"
prompt_file = "prompts/spec-review-rigor.md"
inputs = []
[[workflows.nodes]]
id = "soundness"
agent = "claude"
prompt_file = "prompts/spec-review-soundness.md"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt_file = "prompts/spec-review-synth.md"
inputs = ["rigor", "soundness"]

[[workflows]]
id = "plan-review"
[[workflows.nodes]]
id = "exec"
agent = "codex"
prompt_file = "prompts/plan-review-exec.md"
inputs = []
[[workflows.nodes]]
id = "coverage"
agent = "claude"
prompt_file = "prompts/plan-review-coverage.md"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt_file = "prompts/plan-review-synth.md"
inputs = ["exec", "coverage"]

# design: two clean-room architect lenses (firewalled via inputs=[]) + synth.
[[workflows]]
id = "design"
[[workflows.nodes]]
id = "executability"
agent = "codex"
prompt_file = "prompts/design-executability.md"
inputs = []
[[workflows.nodes]]
id = "structure"
agent = "claude"
prompt_file = "prompts/design-structure.md"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt_file = "prompts/design-synth.md"
inputs = ["executability", "structure"]
"#;

/// Build the `a2a-bridge.toml` contents for the selected agents.
/// `default` is `--default` if given, else `kiro` if selected, else the first
/// selected agent (so the generated default is always a real entry).
fn build_init_config(
    selected: &[String],
    default_override: Option<&str>,
) -> Result<String, BoxError> {
    let known = known_init_agents();
    let default_agent = match default_override {
        Some(d) => {
            if !selected.iter().any(|a| a == d) {
                return Err(format!("init: --default {d:?} is not among --agents").into());
            }
            d.to_string()
        }
        None if selected.iter().any(|a| a == "kiro") => "kiro".to_string(),
        None => selected[0].clone(),
    };
    let allowed: Vec<String> = selected
        .iter()
        .filter_map(|a| known.iter().find(|(n, _)| n == a).and_then(|(_, c)| *c))
        .map(|c| format!("\"{c}\""))
        .collect();

    let mut out = String::new();
    out.push_str("# Generated by `a2a-bridge init`. See README-a2a-bridge.md.\n");
    out.push_str(&format!("default = \"{default_agent}\"\n\n"));
    out.push_str(&format!(
        "[registry]\nallowed_cmds = [{}]\n\n",
        allowed.join(", ")
    ));
    out.push_str("[store]\npath = \".a2a-bridge/tasks.sqlite\"\nresume_attempt_cap = 3\n\n");
    out.push_str("[server]\naddr = \"127.0.0.1:8080\"\n");
    for a in selected {
        out.push_str(agent_fragment(a));
    }
    // Workflows reference codex AND claude; emit only when both are present.
    if selected.iter().any(|a| a == "codex") && selected.iter().any(|a| a == "claude") {
        out.push_str(INIT_WORKFLOWS);
    }
    Ok(out)
}

/// `a2a-bridge init [--dir <path>] [--agents kiro,codex,claude,api] [--default <id>] [--force]`
fn init_cmd(args: &[String]) -> Result<(), BoxError> {
    let dir = PathBuf::from(flag(args, "--dir").unwrap_or("."));
    let agents_csv = flag(args, "--agents")
        .unwrap_or("kiro,codex,claude,api")
        .to_string();
    let default_override = flag(args, "--default");
    let force = args.iter().any(|a| a == "--force");

    let known = known_init_agents();
    let selected: Vec<String> = agents_csv
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if selected.is_empty() {
        return Err("init: --agents must list at least one agent".into());
    }
    for a in &selected {
        if !known.iter().any(|(n, _)| n == a) {
            return Err(
                format!("init: unknown agent {a:?} (known: kiro, codex, claude, api)").into(),
            );
        }
    }

    let config = build_init_config(&selected, default_override)?;

    // Assemble the managed file set: config + README + the 9 prompts.
    let mut files: Vec<(PathBuf, String)> = vec![
        (dir.join("a2a-bridge.toml"), config),
        (dir.join("README-a2a-bridge.md"), INIT_README.to_string()),
    ];
    for (rel, contents) in INIT_PROMPTS {
        files.push((dir.join(rel), contents.to_string()));
    }

    // Clobber guard: refuse if any managed file exists, unless --force.
    if !force {
        let existing: Vec<String> = files
            .iter()
            .filter(|(p, _)| p.exists())
            .map(|(p, _)| p.display().to_string())
            .collect();
        if !existing.is_empty() {
            return Err(format!(
                "init: refusing to overwrite existing files (use --force):\n  {}",
                existing.join("\n  ")
            )
            .into());
        }
    }

    // Write everything (creating parent dirs); never touch unknown files.
    std::fs::create_dir_all(dir.join("prompts"))?;
    std::fs::create_dir_all(dir.join(".a2a-bridge"))?;
    for (path, contents) in &files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
    }

    println!(
        "Initialized a2a-bridge config in {} ({} agent{}: {}).",
        dir.display(),
        selected.len(),
        if selected.len() == 1 { "" } else { "s" },
        selected.join(", ")
    );
    println!(
        "Run: a2a-bridge serve --config {}",
        dir.join("a2a-bridge.toml").display()
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    // Dispatch subcommands BEFORE the server path touches the filesystem.
    let raw_args: Vec<String> = std::env::args().collect();
    match raw_args.get(1).map(|s| s.as_str()) {
        Some("run-workflow") => return run_workflow_cmd(&raw_args[2..]).await,
        Some("implement") => return implement_cmd(&raw_args[2..]).await,
        Some("submit") => return submit_cmd(&raw_args[2..]).await,
        Some("task") => return task_cmd(&raw_args[2..]).await,
        Some("init") => return init_cmd(&raw_args[2..]),
        // `serve` (explicit) and the bare invocation fall through to the server path.
        Some("serve") | None => {}
        // An unknown first token must NOT silently serve (a typo'd subcommand or flag
        // would otherwise be swallowed and the default served).
        Some(other) => {
            return Err(format!(
                "a2a-bridge: unknown subcommand {other:?} (expected: serve | run-workflow | implement | submit | task | init)"
            )
            .into());
        }
    }

    // 1. Observability — install tracing subscriber (idempotent).
    bridge_observ::init();

    // 2. Configuration. `serve --config <path>` reads an EXPLICIT config (must already
    //    exist — an explicit path is a promise, so a missing one errors with an `init`
    //    hint rather than silently materialising a kiro-only file). Bare `a2a-bridge`
    //    (or `serve` with no --config) reads ./a2a-bridge.toml, materialising the
    //    kiro-only DEFAULT_CONFIG if absent (zero-config first run). The path is
    //    absolutised so workflow prompt + relative store paths resolve against the
    //    config's OWN directory, not the process CWD.
    let explicit_config = if raw_args.get(1).map(|s| s.as_str()) == Some("serve") {
        serve_config_flag(&raw_args[2..])?
    } else {
        None
    };
    let config_path = match explicit_config {
        Some(p) => {
            if !p.exists() {
                return Err(format!(
                    "a2a-bridge: config not found at {}; run `a2a-bridge init` to create one",
                    p.display()
                )
                .into());
            }
            p
        }
        None => {
            let p = PathBuf::from(CONFIG_PATH);
            if !p.exists() {
                std::fs::write(&p, DEFAULT_CONFIG)?;
            }
            p
        }
    };
    let config_path = std::fs::canonicalize(&config_path).map_err(|e| {
        format!(
            "a2a-bridge: cannot resolve config path {}: {e}",
            config_path.display()
        )
    })?;

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
    let owner_config_path = config_path.clone();
    let spawn: SpawnFn = Arc::new(move |entry: Arc<AgentEntry>| {
        let policy = Arc::clone(&policy_for_spawn);
        let owner_config_path = owner_config_path.clone();
        Box::pin(async move {
            // The host child has no cwd (Supervised gets None); AcpConfig.cwd IS the ACP session cwd.
            // Resolution chain: session_cwd → cwd → ".". A relative resolved value is joined
            // onto current_dir() to become absolute; an absolute one is used as-is (ACP §11A).
            let resolved =
                resolve_static_session_cwd(entry.session_cwd.as_deref(), entry.cwd.as_deref());
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
            use bridge_core::domain::AgentKind;
            match entry.kind {
                AgentKind::Acp => {
                    // Compose-or-raw + the `:ro` reaper via the shared helper (same as run-workflow).
                    let (program, argv, acp) = acp_spawn_inputs(&entry, cwd, &owner_config_path)?;
                    let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
                    let be = AcpBackend::spawn(&program, &argv_ref, acp)
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
                AgentKind::ContainerRw => {
                    let sb = entry.sandbox.clone().ok_or(BridgeError::ConfigInvalid {
                        reason: format!(
                            "container_rw agent {} requires sandbox",
                            entry.id.as_str()
                        ),
                    })?;
                    let cmd = entry.cmd.clone().ok_or(BridgeError::ConfigInvalid {
                        reason: format!("container_rw agent {} requires cmd", entry.id.as_str()),
                    })?;
                    let owner = container_owner(&owner_config_path, &sb.mount, entry.id.as_str());
                    let ccfg = bridge_container::ContainerRwConfig {
                        sandbox: sb,
                        cmd,
                        args: entry.args.clone(),
                        model: entry.model.clone(),
                        mode: entry.mode.clone(),
                        auth_method: entry.auth_method.clone(),
                        handshake_timeout: bridge_acp::acp_backend::AcpConfig::default()
                            .handshake_timeout,
                        cancel_grace: bridge_acp::acp_backend::AcpConfig::default().cancel_grace,
                    };
                    let cspawn: Arc<dyn bridge_container::ContainerSpawn> =
                        Arc::new(AcpContainerSpawn {
                            policy: Arc::clone(&policy),
                        });
                    let be = bridge_container::ContainerRwBackend::new(ccfg, cspawn, owner).await?;
                    Ok(Arc::new(be) as Arc<dyn bridge_core::ports::AgentBackend>)
                }
            }
        })
    });

    // 5. Config source + registry. `load()` is the initial desired state; the
    //    delegation `[delegation]` env-expansion + `[server] addr` ride along on
    //    the RegistryConfig we re-read below for the non-registry fields.
    let source = FileConfigSource::new(config_path.clone());
    let snapshot = source.load().await?; // initial desired state
                                         // :ro reaper boot-sweep: reap this instance's orphaned :ro
                                         // containers (config_path is already canonical, matching the
                                         // owner the spawn factory uses). serve is long-running, so it
                                         // needs no END-sweep — per-backend retire reaps run with the
                                         // runtime alive, and the next boot-sweep catches any leftover.
    ro_sweep(&ro_sweep_targets(&snapshot, &config_path));
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
                // Resolve a RELATIVE store path against the config's own directory
                // (`base`), not the process CWD — so `serve --config /elsewhere/...`
                // keeps task state beside its config, not wherever serve was launched.
                let store_path = {
                    let p = std::path::Path::new(&path);
                    if p.is_absolute() {
                        p.to_path_buf()
                    } else {
                        base.join(p)
                    }
                };
                let s =
                    std::sync::Arc::new(SqliteStore::open(&store_path).map_err(|e| {
                        format!("serve: cannot open task store {store_path:?}: {e:?}")
                    })?);
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

    fn acp_entry(id: &str) -> AgentEntry {
        use bridge_core::ids::AgentId;
        use std::collections::BTreeMap;
        AgentEntry {
            id: AgentId::parse(id).unwrap(),
            cmd: Some("claude-agent-acp".into()),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind: bridge_core::domain::AgentKind::Acp,
            model_provider: None,
            model: None,
            effort: None,
            mode: None,
            cwd: None,
            session_cwd: None,
            sandbox: None,
            auth_method: None,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            extensions: BTreeMap::new(),
        }
    }

    #[test]
    fn acp_program_argv_raw_passthrough_and_sandbox_wrap() {
        use bridge_core::domain::{EgressPolicy, MountAccess, SandboxConfig};
        // raw: program = cmd, argv = args (Slice A compat).
        let raw = acp_entry("a");
        assert_eq!(
            acp_program_argv(&raw, None).unwrap(),
            ("claude-agent-acp".to_string(), Vec::<String>::new())
        );
        // sandbox: program = runtime (docker), argv wraps the inner cli with the :ro mount.
        let mut sb = acp_entry("b");
        sb.sandbox = Some(SandboxConfig {
            runtime: None,
            image: "img".into(),
            mount: "/work".into(),
            access: MountAccess::Ro,
            egress: EgressPolicy::Open,
            volumes: vec![],
        });
        let (program, argv) = acp_program_argv(&sb, None).unwrap();
        assert_eq!(program, "docker");
        assert_eq!(argv.last().unwrap(), "claude-agent-acp");
        assert!(argv.contains(&"/work:/work:ro".to_string()));
        // sandbox + a container name → the `--name` is spliced in (the :ro reaper path).
        let (_p, named) = acp_program_argv(&sb, Some("a2a-ro-owner-nonce")).unwrap();
        assert!(named
            .windows(2)
            .any(|w| w == ["--name", "a2a-ro-owner-nonce"]));
        // missing cmd → ConfigInvalid.
        let mut nocmd = acp_entry("c");
        nocmd.cmd = None;
        assert!(acp_program_argv(&nocmd, None).is_err());
    }

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

    #[test]
    fn serve_config_flag_parses_and_rejects_unknown() {
        assert_eq!(serve_config_flag(&[]).unwrap(), None);
        let a = vec!["--config".to_string(), "/x/a2a-bridge.toml".to_string()];
        assert_eq!(
            serve_config_flag(&a).unwrap(),
            Some(PathBuf::from("/x/a2a-bridge.toml"))
        );
        assert!(serve_config_flag(&["--config".to_string()]).is_err()); // missing value
        assert!(serve_config_flag(&["--bogus".to_string()]).is_err()); // unknown flag
    }

    #[test]
    fn init_default_resolution_and_allowed_cmds() {
        // kiro present -> default kiro; allowed_cmds = the ACP cmds (api excluded).
        let all = vec![
            "kiro".to_string(),
            "codex".to_string(),
            "claude".to_string(),
            "api".to_string(),
        ];
        let cfg = build_init_config(&all, None).unwrap();
        assert!(cfg.contains("default = \"kiro\""));
        assert!(cfg.contains(r#"allowed_cmds = ["kiro-cli", "codex-acp", "claude-agent-acp"]"#));
        // kiro excluded -> default falls to the first selected agent (codex), not a dangling kiro.
        let codex_only = vec!["codex".to_string()];
        let cfg = build_init_config(&codex_only, None).unwrap();
        assert!(cfg.contains("default = \"codex\""));
        // --default override must be among the selected agents.
        assert!(build_init_config(&codex_only, Some("claude")).is_err());
        assert!(build_init_config(&codex_only, Some("codex")).is_ok());
    }

    #[test]
    fn init_workflows_only_when_codex_and_claude_present() {
        // Both -> the review workflows are emitted.
        let both = vec!["codex".to_string(), "claude".to_string()];
        assert!(build_init_config(&both, None)
            .unwrap()
            .contains("[[workflows]]"));
        // Missing one -> NO workflows (else load_workflows fails on a missing agent).
        let codex_only = vec!["codex".to_string()];
        assert!(!build_init_config(&codex_only, None)
            .unwrap()
            .contains("[[workflows]]"));
    }

    #[test]
    fn init_generated_config_parses_and_loads() {
        // End-to-end: init writes config + prompts; the generated config parses AND
        // its workflows load (prompt paths resolve relative to the config dir).
        let dir = std::env::temp_dir().join(format!("a2a-init-test-gen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        init_cmd(&[
            "--dir".to_string(),
            dir.to_string_lossy().to_string(),
            "--agents".to_string(),
            "kiro,codex,claude".to_string(),
        ])
        .unwrap();
        let raw = std::fs::read_to_string(dir.join("a2a-bridge.toml")).unwrap();
        let cfg = config::RegistryConfig::parse(&raw).unwrap();
        let wf = cfg.load_workflows(&dir).unwrap();
        assert_eq!(
            wf.len(),
            5,
            "code-review + implement-review + spec-review + plan-review + design load"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_clobber_guard_and_force_and_unknown_agent() {
        let dir = std::env::temp_dir().join(format!("a2a-init-test-force-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let args = |extra: &[&str]| {
            let mut v = vec!["--dir".to_string(), dir.to_string_lossy().to_string()];
            v.extend(extra.iter().map(|s| s.to_string()));
            v
        };
        // First init succeeds.
        init_cmd(&args(&["--agents", "kiro"])).unwrap();
        // Second init REFUSES (would clobber the managed files).
        assert!(init_cmd(&args(&["--agents", "kiro"])).is_err());
        // ...but --force overwrites the managed set.
        init_cmd(&args(&["--agents", "kiro", "--force"])).unwrap();
        // An unknown agent name errors BEFORE writing anything.
        let bad = std::env::temp_dir().join(format!("a2a-init-test-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bad);
        assert!(init_cmd(&[
            "--dir".to_string(),
            bad.to_string_lossy().to_string(),
            "--agents".to_string(),
            "kiro,bogus".to_string(),
        ])
        .is_err());
        assert!(
            !bad.join("a2a-bridge.toml").exists(),
            "no files written on bad --agents"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bad);
    }

    #[test]
    fn reference_multi_agent_config_parses_and_loads() {
        // The committed examples/ config parses + its workflows load via ../prompts/.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/a2a-bridge.multi-agent.toml");
        let raw = std::fs::read_to_string(&path).unwrap();
        let cfg = config::RegistryConfig::parse(&raw).unwrap();
        let base = path.parent().unwrap();
        let wf = cfg.load_workflows(base).unwrap();
        assert_eq!(wf.len(), 4);
    }

    // ---- Task 10: task watch <id> arg-parsing ----

    /// `task watch <id>` with no optional flags: id parsed, url defaults, from is None.
    #[test]
    fn task_watch_parses_id_only() {
        // Simulate the args slice that task_cmd receives: ["watch", "<id>"]
        let args = vec!["watch".to_string(), "task-abc-123".to_string()];
        let sub = args.first().map(|s| s.as_str()).unwrap();
        assert_eq!(sub, "watch");

        let id = args.get(1).cloned().expect("id present");
        assert_eq!(id, "task-abc-123");

        let url = flag(&args, "--url").unwrap_or("http://127.0.0.1:8080");
        assert_eq!(url, "http://127.0.0.1:8080");

        let from: Option<i64> = flag(&args, "--from").and_then(|s| s.parse().ok());
        assert!(from.is_none());
    }

    /// `task watch <id> --url <url>` overrides the default url.
    #[test]
    fn task_watch_parses_url_override() {
        let args = vec![
            "watch".to_string(),
            "task-abc-123".to_string(),
            "--url".to_string(),
            "http://10.0.0.1:9090".to_string(),
        ];
        let id = args.get(1).cloned().expect("id present");
        assert_eq!(id, "task-abc-123");

        let url = flag(&args, "--url").unwrap_or("http://127.0.0.1:8080");
        assert_eq!(url, "http://10.0.0.1:9090");
    }

    /// `task watch <id> --from <seq>` parses the cursor as i64.
    #[test]
    fn task_watch_parses_from_cursor() {
        let args = vec![
            "watch".to_string(),
            "task-abc-123".to_string(),
            "--from".to_string(),
            "42".to_string(),
        ];
        let from: Option<i64> = flag(&args, "--from").and_then(|s| s.parse().ok());
        assert_eq!(from, Some(42));
    }

    /// `task watch <id> --url <u> --from <seq>` — all three fields parse together.
    #[test]
    fn task_watch_parses_all_fields() {
        let args = vec![
            "watch".to_string(),
            "task-xyz".to_string(),
            "--url".to_string(),
            "http://bridge:8080".to_string(),
            "--from".to_string(),
            "7".to_string(),
        ];
        let id = args.get(1).cloned().expect("id");
        let url = flag(&args, "--url").unwrap_or("http://127.0.0.1:8080");
        let from: Option<i64> = flag(&args, "--from").and_then(|s| s.parse().ok());

        assert_eq!(id, "task-xyz");
        assert_eq!(url, "http://bridge:8080");
        assert_eq!(from, Some(7));
    }

    /// Missing `<id>` after `watch` returns an error (mirrors `task get` behaviour).
    #[test]
    fn task_watch_missing_id_is_error() {
        let args = ["watch".to_string()];
        let result = args.get(1).cloned().ok_or("task watch: missing <id>");
        assert!(result.is_err());
    }

    #[test]
    fn parse_run_workflow_args_session_cwd() {
        let args: Vec<String> = ["wf", "--input", "in.md", "--session-cwd", "/work/repo"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (id, input, _out, _cfg, scwd) = super::parse_run_workflow_args(&args).unwrap();
        assert_eq!(id, "wf");
        assert_eq!(input, std::path::PathBuf::from("in.md"));
        assert_eq!(scwd.as_deref(), Some("/work/repo"));
    }

    #[test]
    fn parse_run_workflow_args_no_session_cwd_is_none() {
        let args: Vec<String> = ["wf", "--input", "in.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (_id, _input, _out, _cfg, scwd) = super::parse_run_workflow_args(&args).unwrap();
        assert!(scwd.is_none());
    }

    #[test]
    fn parse_implement_args_basic() {
        let a: Vec<String> = [
            "Add a FOO file",
            "--repo",
            "/src/repo",
            "--base-ref",
            "main",
            "--config",
            "c.toml",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let p = super::parse_implement_args(&a).unwrap();
        assert_eq!(p.task, "Add a FOO file");
        assert_eq!(p.repo, std::path::PathBuf::from("/src/repo"));
        assert_eq!(p.base_ref.as_deref(), Some("main"));
        assert_eq!(p.config, std::path::PathBuf::from("c.toml"));
        assert_eq!(p.workflow, "implement-edit"); // default
    }

    #[test]
    fn parse_implement_args_requires_task_and_repo() {
        // first token is a flag -> treated as missing <task>
        assert!(super::parse_implement_args(&["--repo".into(), "/r".into()]).is_err());
        // task present but no --repo
        assert!(super::parse_implement_args(&["task".into()]).is_err());
    }

    // R11: the example containerized config (the `impl` ContainerRw agent + the implement-edit workflow)
    // parses, the workflow loads, into_snapshot succeeds, and Registry::new validates it — Docker-free.
    #[test]
    fn containerized_config_validates_with_implement_edit() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let raw =
            std::fs::read_to_string(root.join("examples/a2a-bridge.containerized.toml")).unwrap();
        let cfg = config::RegistryConfig::parse(&raw).unwrap();
        let wf = cfg.load_workflows(&root.join("examples")).unwrap();
        assert!(
            wf.contains_key(&bridge_core::ids::WorkflowId::parse("implement-edit").unwrap()),
            "implement-edit workflow loads"
        );
        let snap = cfg.into_snapshot().unwrap();
        // Registry::new validates the snapshot WITHOUT spawning (lazy), so the real make_spawn_fn is never
        // called here — reuse it to avoid hand-rolling a typed no-op SpawnFn.
        let policy: std::sync::Arc<dyn bridge_core::ports::PolicyEngine> =
            std::sync::Arc::new(AutoPolicy);
        let spawn =
            super::make_spawn_fn(policy, root.join("examples/a2a-bridge.containerized.toml"));
        bridge_registry::registry::Registry::new(snap, spawn)
            .expect("containerized config (incl. the impl container_rw agent) validates");
    }
}
