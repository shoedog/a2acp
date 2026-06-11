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
mod containers;
mod implement;
mod implement_resume;
mod merge;
mod resilient;
mod review;
mod route;
mod turn;
mod tweak;
mod verify;

use std::path::{Path, PathBuf};
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

/// Top-level usage, printed by `a2a-bridge help|--help|-h`. The detailed flags live in each subcommand's
/// `--help`; the copy-paste quickstart lives in `AGENTS.md`.
const TOP_USAGE: &str = "\
a2a-bridge — A2A↔ACP bridge + multi-agent workflow runner

USAGE:
  a2a-bridge <subcommand> [options]      (bare `a2a-bridge` or `serve` runs the A2A server)

SUBCOMMANDS:
  run-workflow <id>   Run a workflow against a repo (design | code-review | spec-review | plan-review | …).
                      --input <file> --session-cwd <repo> [--config <f>] [--out <f>]
  implement <task>    Clone a repo, implement the task on a warm containerized agent, verify+review, hand off.
                      --repo <path> [--config <f>] [--base-ref <ref>] [--workflow <id>] [--merge [--onto <branch>]]
  merge <id>          Land an Approved run's commit into its source repo, re-authored to the operator
                      (Mode A: fast-forward --onto). [--config <f>] [--onto <branch>] [--force]
  init                Scaffold an a2a-bridge.toml + prompts.  --agents codex,claude [--dir <d>] [--force]
  serve               Run the A2A server.  [--config <path>]
  containers          List / reap this config's managed containers (crash-orphan cleanup).  list | reap
  submit | task       Detached workflow submit + durable task store.

Run `a2a-bridge <subcommand> --help` for details. Quickstart + cwd/creds/concurrency notes: AGENTS.md.";

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
    labels: &[(String, String)],
    mcp_cwd: &str,
) -> Result<(String, Vec<String>), BridgeError> {
    let cmd = entry.cmd.as_deref().ok_or(BridgeError::ConfigInvalid {
        reason: format!("acp agent {} missing cmd", entry.id.as_str()),
    })?;
    // Native MCP delivery to the argv (ADR-0028): codex gets `-c mcp_servers.*` overrides; kiro gets
    // `--agent <name>` pointing at the agent-config the bridge writes in `acp_spawn_inputs`. claude
    // (Acp) gets MCP via the session/new param, not here. Empty `mcp` → unchanged args.
    use bridge_core::mcp::McpDelivery;
    let args = if entry.mcp.is_empty() {
        entry.args.clone()
    } else {
        match entry.mcp_delivery {
            McpDelivery::CodexNative => {
                let mut a = entry.args.clone();
                a.extend(bridge_core::mcp::render_codex_mcp_args(&entry.mcp, mcp_cwd));
                a
            }
            McpDelivery::KiroNative => {
                // `--agent` follows the `acp` subcommand already in `entry.args`; the named config
                // (with prism) is written host-side at spawn. kiro MCP is host-only (config guard).
                let mut a = entry.args.clone();
                a.push("--agent".to_string());
                a.push(bridge_core::mcp::kiro_agent_name(entry.id.as_str()));
                a
            }
            McpDelivery::Acp => entry.args.clone(),
        }
    };
    Ok(match (&entry.sandbox, container_name) {
        // Named (`:ro` reaper) container when the caller supplied a name.
        (Some(sb), Some(name)) => {
            bridge_core::sandbox::compose_sandbox_named(sb, name, cmd, &args, labels)
        }
        (Some(sb), None) => bridge_core::sandbox::compose_sandbox(sb, cmd, &args, labels),
        (None, _) => (cmd.to_string(), args),
    })
}

/// Write the bridge-managed kiro agent-config to `~/.kiro/agents/<name>.json` for `KiroNative` delivery
/// (ADR-0028). Overwrites a stable per-agent name each spawn (no cleanup — a benign managed config).
/// `{cwd}` is the spawn cwd; kiro is host-only for MCP (the config guard rejects `KiroNative` + sandbox).
fn write_kiro_agent_config(entry: &AgentEntry, cwd: &str) -> Result<(), BridgeError> {
    let name = bridge_core::mcp::kiro_agent_name(entry.id.as_str());
    let home = std::env::var("HOME").map_err(|_| BridgeError::ConfigInvalid {
        reason: "kiro MCP delivery needs $HOME to locate ~/.kiro/agents".into(),
    })?;
    let dir = std::path::Path::new(&home).join(".kiro").join("agents");
    std::fs::create_dir_all(&dir).map_err(|e| BridgeError::ConfigInvalid {
        reason: format!("kiro agents dir {dir:?}: {e}"),
    })?;
    let path = dir.join(format!("{name}.json"));
    let cfg = bridge_core::mcp::render_kiro_agent_config(&entry.mcp, cwd, &name);
    std::fs::write(&path, cfg).map_err(|e| BridgeError::ConfigInvalid {
        reason: format!("write kiro agent config {path:?}: {e}"),
    })?;
    Ok(())
}

/// Build `(program, argv, AcpConfig)` for a `kind=acp` agent, attaching the `:ro` container reaper when the
/// sandbox is `access=ro` (owner-scoped name `a2a-ro-<owner>-<nonce>` + a `docker rm -f` reap_fn). Shared by
/// BOTH spawn factories (`make_spawn_fn` + `serve`) so the `:ro` naming/reaping can't diverge.
fn acp_spawn_inputs(
    entry: &AgentEntry,
    cwd: PathBuf,
    owner_config_path: &std::path::Path,
    run: &bridge_core::run_identity::RunHandle,
) -> Result<(String, Vec<String>, bridge_acp::acp_backend::AcpConfig), BridgeError> {
    use bridge_core::domain::MountAccess;
    // Increment A: a `:ro` reader carries the run-id in its name (no same-owner concurrent clash) + the
    // full managed label set (so `recover_orphans`/`containers` classify it). `repo`/`cwd` are display-only.
    let cwd_str = cwd.to_string_lossy().to_string();
    // prism's CPG cache is keyed by the --repo PATH: a NON-canonical path hashes to a different (cold,
    // possibly stale) entry. Canonicalize the cwd used for MCP `{cwd}` so the agent deterministically
    // hits the warmed entry (warm the SAME canonical path). The `:rw` implementor already does this via
    // `rw_canon`. Falls back to the raw cwd if canonicalize fails (e.g. unit tests, missing dir).
    let mcp_cwd = std::fs::canonicalize(&cwd)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| cwd_str.clone());
    // kiro MCP delivery (ADR-0028): write the named agent-config (prism, {cwd}-substituted) to
    // ~/.kiro/agents/<name>.json BEFORE spawn; `acp_program_argv` points kiro at it via `--agent`.
    if matches!(
        entry.mcp_delivery,
        bridge_core::mcp::McpDelivery::KiroNative
    ) && !entry.mcp.is_empty()
    {
        write_kiro_agent_config(entry, &mcp_cwd)?;
    }
    let (ro_name, labels) = match entry
        .sandbox
        .as_ref()
        .filter(|sb| matches!(sb.access, MountAccess::Ro))
    {
        Some(sb) => {
            let owner = container_owner(owner_config_path, &sb.mount, entry.id.as_str());
            let name = bridge_core::sandbox::a2a_name(
                "ro",
                &owner,
                &run.instance_id,
                &implement::nonce(8),
            );
            let labels = run
                .labels(
                    "ro",
                    "oneshot",
                    entry.id.as_str(),
                    &owner,
                    Some(cwd_str.as_str()),
                    Some(cwd_str.as_str()),
                )
                .to_arg_pairs();
            (Some(name), labels)
        }
        None => (None, Vec::new()),
    };
    let (program, argv) = acp_program_argv(entry, ro_name.as_deref(), &labels, &mcp_cwd)?;
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
        agent_id: entry.id.as_str().to_string(),
        cwd,
        model: entry.model.clone(),
        mode: entry.mode.clone(),
        auth_method: entry.auth_method.clone(),
        container,
        // ACP-param MCP delivery (claude): the entry's MCP servers ride `session/new`. Codex/kiro
        // native delivery leaves this empty (they get MCP via their native channel, not the param).
        mcp: if matches!(entry.mcp_delivery, bridge_core::mcp::McpDelivery::Acp) {
            entry.mcp.clone()
        } else {
            Vec::new()
        },
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

/// Epoch-seconds string for the display-only `a2a.start` label (no RFC3339 dep; `containers list` shows
/// `age = now - start`).
fn epoch_secs() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
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

/// `(runtime, owner)` sweep targets for THIS instance's `:rw` (ContainerRw) agents — mirrors
/// [`ro_sweep_targets`]. The owner is computed from the SAME `(config_path, mount, agent_id)` triple as
/// the warm backend's spawn-time owner (via [`container_owner`]), so the guard sweeps the right containers
/// (spec §5 silent-leak guard).
fn rw_sweep_targets(
    snapshot: &bridge_core::domain::RegistrySnapshot,
    config_path: &std::path::Path,
) -> Vec<(String, String)> {
    use bridge_core::domain::AgentKind;
    let mut targets = Vec::new();
    for entry in &snapshot.entries {
        let Some(sb) = entry.sandbox.as_ref() else {
            continue;
        };
        if entry.kind != AgentKind::ContainerRw {
            continue;
        }
        let owner = container_owner(config_path, &sb.mount, entry.id.as_str());
        targets.push((sb.runtime().to_string(), owner));
    }
    targets
}

/// Increment A before-first-use crash-orphan recovery (replaces the owner-name boot sweeps). For every
/// owner THIS process can spawn (`:rw` ContainerRw ∪ `:ro` sandboxed Acp), [`classify_sweep`] inspects the
/// owner's MANAGED containers and reaps ONLY the DEAD ones (same host + a FREE flock lease). A live
/// concurrent run holds its lease → its containers classify Alive → spared. Idempotent + best-effort, so
/// it's safe to call at every entry point (one-shots once; serve at startup AND on each hot-reload).
fn recover_orphans(
    snapshot: &bridge_core::domain::RegistrySnapshot,
    config_path: &std::path::Path,
    host: &str,
) {
    use bridge_core::liveness::FsLeaseProbe;
    let mut owners: Vec<(String, String)> = Vec::new();
    owners.extend(rw_sweep_targets(snapshot, config_path));
    owners.extend(ro_sweep_targets(snapshot, config_path));
    owners.sort();
    owners.dedup();
    // Reap every owner's DEAD orphans FIRST, collecting their (shared) lease files; delete the leases ONCE
    // at the end. A crashed run's containers span multiple owners but share one lease — deleting it per-owner
    // would blind the later owners' sweeps (absent lease → Unknown → spared → leak; live-gate finding).
    let mut dead_leases: Vec<String> = Vec::new();
    for (runtime, owner) in owners {
        dead_leases.extend(bridge_core::reaper::classify_sweep(
            &runtime,
            &owner,
            host,
            &FsLeaseProbe,
        ));
    }
    dead_leases.sort();
    dead_leases.dedup();
    for lease in dead_leases {
        let _ = std::fs::remove_file(&lease);
    }
}

/// The DISTINCT container runtimes this process uses (across its `:rw` + `:ro` owners) — the set the
/// [`RunEndGuard`] must sweep this run's `a2a.run` label over.
fn run_guard_runtimes(
    snapshot: &bridge_core::domain::RegistrySnapshot,
    config_path: &std::path::Path,
) -> Vec<String> {
    let mut rts: Vec<String> = rw_sweep_targets(snapshot, config_path)
        .into_iter()
        .chain(ro_sweep_targets(snapshot, config_path))
        .map(|(runtime, _owner)| runtime)
        .collect();
    rts.sort();
    rts.dedup();
    rts
}

/// RAII END-sweep for one-shot commands (`implement`/`run-workflow`): on drop (any return path) it
/// synchronously reaps THIS run's containers — both `:rw` and `:ro` — by the `a2a.run` label, across every
/// runtime in use. Label-scoped (not owner-name), so a CONCURRENT run's containers (a different `a2a.run`)
/// are NEVER touched. Synchronous because the per-backend Drop reaper detaches its `docker rm -f`, which
/// races process exit on a one-shot (the runtime can die before the detached task runs — live-gate finding).
/// Declared BEFORE the warm backend so it drops AFTER it (warm `retire` reaps first; this is the backstop).
struct RunEndGuard {
    runtimes: Vec<String>,
    instance_id: String,
}
impl Drop for RunEndGuard {
    fn drop(&mut self) {
        for runtime in &self.runtimes {
            bridge_core::reaper::run_scoped_reap(runtime, &self.instance_id);
        }
    }
}

/// Build a [`bridge_container::ContainerRwConfig`] from a ContainerRw agent entry — shared by
/// [`make_spawn_fn`] (per-turn) and the warm `implement` path so both compose the SAME container.
fn container_rw_cfg_from_entry(
    entry: &AgentEntry,
    run: &bridge_core::run_identity::RunHandle,
) -> Result<bridge_container::ContainerRwConfig, BridgeError> {
    let sb = entry.sandbox.clone().ok_or(BridgeError::ConfigInvalid {
        reason: format!("container_rw agent {} requires sandbox", entry.id.as_str()),
    })?;
    let cmd = entry.cmd.clone().ok_or(BridgeError::ConfigInvalid {
        reason: format!("container_rw agent {} requires cmd", entry.id.as_str()),
    })?;
    Ok(bridge_container::ContainerRwConfig {
        sandbox: sb,
        cmd,
        args: entry.args.clone(),
        mcp: entry.mcp.clone(),
        mcp_delivery: entry.mcp_delivery,
        model: entry.model.clone(),
        mode: entry.mode.clone(),
        auth_method: entry.auth_method.clone(),
        handshake_timeout: bridge_acp::acp_backend::AcpConfig::default().handshake_timeout,
        cancel_grace: bridge_acp::acp_backend::AcpConfig::default().cancel_grace,
        run: run.clone(),
        agent: entry.id.as_str().to_string(),
    })
}

/// The production `SpawnFn` (Acp compose-or-raw / Api / ContainerRw arms) — shared by run-workflow and the
/// `implement` subcommand so their registry builds can't drift. `owner_config_path` seeds the ContainerRw
/// owner token.
fn make_spawn_fn(
    policy_for_spawn: Arc<dyn bridge_core::ports::PolicyEngine>,
    owner_config_path: PathBuf,
    run: bridge_core::run_identity::RunHandle,
) -> bridge_registry::registry::SpawnFn {
    Arc::new(move |entry: Arc<AgentEntry>| {
        let policy = Arc::clone(&policy_for_spawn);
        let owner_config_path = owner_config_path.clone();
        let run = run.clone();
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
                    let (program, argv, acp) =
                        acp_spawn_inputs(&entry, cwd, &owner_config_path, &run)?;
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
                    let owner = {
                        let sb = entry.sandbox.as_ref().ok_or(BridgeError::ConfigInvalid {
                            reason: format!(
                                "container_rw agent {} requires sandbox",
                                entry.id.as_str()
                            ),
                        })?;
                        container_owner(&owner_config_path, &sb.mount, entry.id.as_str())
                    };
                    let ccfg = container_rw_cfg_from_entry(&entry, &run)?;
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
const RUN_WORKFLOW_USAGE: &str = "\
usage: a2a-bridge run-workflow <workflow-id> --input <file> [--session-cwd <repo>] [--config <path>] [--out <file>]
  <workflow-id>   design | code-review | spec-review | plan-review | … (whatever your --config defines)
  --input <file>  the problem statement / material the workflow acts on (required)
  --session-cwd   the repo the agents read/work in (per-request cwd; without it they use the launch cwd)
  --config <path> registry config (default: ./a2a-bridge.toml)
  --out <file>    write the terminal node's output here (default: stdout)";

#[allow(clippy::type_complexity)]
fn parse_run_workflow_args(
    args: &[String],
) -> Result<(String, PathBuf, Option<PathBuf>, PathBuf, Option<String>), BoxError> {
    let mut iter = args.iter().peekable();
    // Positional: workflow id
    let workflow_id = iter
        .next()
        .cloned()
        .ok_or_else(|| format!("run-workflow: missing <workflow-id>\n{RUN_WORKFLOW_USAGE}"))?;
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
            other => {
                return Err(
                    format!("run-workflow: unknown flag {other:?}\n{RUN_WORKFLOW_USAGE}").into(),
                );
            }
        }
    }
    let input = input
        .ok_or_else(|| format!("run-workflow: --input <file> is required\n{RUN_WORKFLOW_USAGE}"))?;
    let config = config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH));
    Ok((workflow_id, input, out, config, session_cwd))
}

// ---------------------------------------------------------------------------
// `implement` subcommand (Slice B2b-1)
// ---------------------------------------------------------------------------

enum ImplementMode {
    Fresh {
        task: String,
        repo: PathBuf,
        base_ref: Option<String>,
        workflow: String,
    },
    Resume {
        resume_id: String,
    },
}

struct ImplementArgs {
    mode: ImplementMode,
    config: PathBuf,
    /// `--merge`: after the run, land an Approved result into source_repo (ADR-0027). Approved-only sugar.
    merge: bool,
    /// `--onto <branch>`: the merge target (when `--merge`); else `[merge].target_ref` / `base_ref`.
    onto: Option<String>,
}

const IMPLEMENT_USAGE: &str = "\
usage: a2a-bridge implement <task> --repo <path> [--config <path>] [--base-ref <ref>] [--workflow <id>]
       a2a-bridge implement --resume <id> [--config <path>]
  <task>          what to implement (a sentence/paragraph the agent acts on)
  --repo <path>   the repo to implement in; cloned into a quarantine under allowed_cwd_root (required)
  --config <path> registry config defining the impl agent + [implement]/[verify]/[review] (default: ./a2a-bridge.toml)
  --base-ref      branch/SHA to start from (default: the repo HEAD)
  --workflow <id> the edit workflow (default: implement-edit)
  --resume <id>   resume a stranded run by its <id> (the clone dir name)
Clones --repo, runs the warm containerized impl agent (edit+fix turns share one container+session),
verifies, reviews the diff, and hands off a branch to merge.";

fn parse_implement_args(args: &[String]) -> Result<ImplementArgs, BoxError> {
    if args.first().map(String::as_str) == Some("--resume") {
        let resume_id = args
            .get(1)
            .cloned()
            .ok_or("implement: --resume needs an <id>")?;
        let mut config = None;
        let mut merge = false;
        let mut onto = None;
        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "--config" => {
                    config = Some(PathBuf::from(
                        args.get(i + 1).ok_or("implement: --config needs a value")?,
                    ));
                    i += 2;
                }
                "--merge" => {
                    merge = true;
                    i += 1;
                }
                "--onto" => {
                    onto = Some(
                        args.get(i + 1)
                            .ok_or("implement: --onto needs a value")?
                            .clone(),
                    );
                    i += 2;
                }
                other => {
                    return Err(format!(
                        "implement --resume: unexpected arg {other:?}\n{IMPLEMENT_USAGE}"
                    )
                    .into());
                }
            }
        }
        return Ok(ImplementArgs {
            mode: ImplementMode::Resume { resume_id },
            config: config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH)),
            merge,
            onto,
        });
    }

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
    let mut merge = false;
    let mut onto = None;
    while let Some(f) = iter.next() {
        match f.as_str() {
            "--merge" => merge = true,
            "--onto" => {
                onto = Some(
                    iter.next()
                        .ok_or("implement: --onto needs a value")?
                        .clone(),
                )
            }
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
                return Err(format!("implement: unknown flag {other:?}\n{IMPLEMENT_USAGE}").into());
            }
        }
    }
    Ok(ImplementArgs {
        mode: ImplementMode::Fresh {
            task,
            repo: repo.ok_or_else(|| {
                format!("implement: --repo <path> is required\n{IMPLEMENT_USAGE}")
            })?,
            base_ref,
            workflow: workflow.unwrap_or_else(|| "implement-edit".into()),
        },
        config: config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH)),
        merge,
        onto,
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

/// Resolve the single ContainerRw agent that drives BOTH the edit and fix turns of one warm session, or a
/// fail-loud reason. Edit & fix workflows must each be single-node and name the SAME ContainerRw agent (one
/// warm container/session can't serve two agents). Pure + Docker-free (validated pre-first-commit).
fn resolve_impl_identity(
    edit_graph: &bridge_workflow::graph::WorkflowGraph,
    fix_graph: Option<&bridge_workflow::graph::WorkflowGraph>,
    snapshot: &bridge_core::domain::RegistrySnapshot,
) -> Result<bridge_core::domain::AgentEntry, String> {
    let edit_node = match edit_graph.nodes.as_slice() {
        [n] => n,
        _ => return Err("edit workflow must be single-node for the warm session".into()),
    };
    let id = &edit_node.agent;
    if let Some(fg) = fix_graph {
        match fg.nodes.as_slice() {
            [n] if &n.agent == id => {}
            [_] => {
                return Err(
                    "fix workflow agent must match the edit agent (one warm session)".into(),
                );
            }
            _ => return Err("fix workflow must be single-node".into()),
        }
    }
    let entry = snapshot
        .entries
        .iter()
        .find(|e| &e.id == id)
        .ok_or_else(|| format!("impl agent {} not found in snapshot", id.as_str()))?
        .clone();
    if entry.kind != bridge_core::domain::AgentKind::ContainerRw {
        return Err(format!(
            "warm session requires a container_rw impl agent, got {:?}",
            entry.kind
        ));
    }
    Ok(entry)
}

/// Run the B2b-2 verify once (total). `verify_cfg` was captured pre-snapshot. The verdict run itself never
/// fails (a runner error becomes a failed result); a config error reduces to `ConfigError`.
fn run_verify_step(
    verify_cfg: &Option<Result<config::VerifyConfig, config::ConfigError>>,
    clone_cwd: &bridge_core::SessionCwd,
    repo: &std::path::Path,
) -> verify::VerifyOutcome {
    match verify_cfg {
        None => verify::VerifyOutcome::NotConfigured,
        Some(Err(e)) => {
            eprintln!("[implement] verify: config error: {e:?} — skipping verify");
            verify::VerifyOutcome::ConfigError
        }
        Some(Ok(vcfg)) => {
            let repo_canon = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
            let cache_vol = verify::cache_volume_name(&vcfg.cache, &repo_canon.to_string_lossy());
            eprintln!(
                "[implement] verify: running {} command(s) in {}",
                vcfg.commands.len(),
                vcfg.image
            );
            let verdict = verify::run_verify(
                vcfg,
                clone_cwd,
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
    }
}

/// Pure: the runtimes whose `probe` reports them unavailable. Injectable for tests.
fn missing_runtimes(
    runtimes: &std::collections::BTreeSet<String>,
    probe: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    runtimes.iter().filter(|rt| !probe(rt)).cloned().collect()
}

/// Production probe: `<runtime> info` exits 0 within a bound. BOUNDED so a wedged probe (half-started
/// `podman machine`) can't hang startup — on timeout it kills the child and reports "unavailable" (a warn,
/// not enforcement). Only ever invoked on an ALLOWLISTED runtime (see `preflight_runtimes`), never on a
/// config-named binary the allowlist would reject.
fn runtime_responds(runtime: &str) -> bool {
    let mut child = match std::process::Command::new(runtime)
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
}

/// Warn-level preflight (NOT enforcement — the allowlist/S3 + the verify gate are the hard gates). Collects
/// the distinct container runtimes a snapshot's sandboxes (+ an optional, already-gated verify runtime) use,
/// **restricts to allowlisted runtimes**, probes each (bounded), and warns on any that don't respond. The
/// allowlist restriction is a SECURITY requirement: probing a non-allowlisted, config-named runtime would
/// EXECUTE that binary outside the allowlist — instead such a runtime is left for S3/the gate to reject
/// loudly. A host-only config (no sandboxes, allowlisted verify) probes nothing.
fn preflight_runtimes(
    snapshot: &bridge_core::domain::RegistrySnapshot,
    verify_runtime: Option<&str>,
) {
    let runtimes = runtimes_to_probe(snapshot, verify_runtime);
    for rt in missing_runtimes(&runtimes, &runtime_responds) {
        tracing::warn!(
            runtime = %rt,
            "configured container runtime '{rt}' did not respond to `{rt} info` — is it installed and (for podman) is `podman machine` started?"
        );
    }
}

/// The runtimes preflight may probe: the distinct sandbox runtimes (+ an optional already-gated verify
/// runtime), RESTRICTED to the allowlist. PURE — the security boundary (never probe a non-allowlisted,
/// config-named binary) is unit-tested here.
fn runtimes_to_probe(
    snapshot: &bridge_core::domain::RegistrySnapshot,
    verify_runtime: Option<&str>,
) -> std::collections::BTreeSet<String> {
    let mut runtimes: std::collections::BTreeSet<String> = snapshot
        .entries
        .iter()
        .filter_map(|e| e.sandbox.as_ref().map(|sb| sb.runtime().to_string()))
        .collect();
    if let Some(rt) = verify_runtime {
        runtimes.insert(rt.to_string());
    }
    runtimes.retain(|rt| snapshot.allowed_cmds.iter().any(|c| c == rt));
    runtimes
}

/// Run the B2b-3a review once (total). Returns `(outcome, synth_body)`. Fresh `CancellationToken` + `select!`
/// timeout→cancel→keep-drain PER call (so the `:ro` reaper still fires on a timed-out attempt). `run_id` is
/// qualified by `attempt`.
#[allow(clippy::too_many_arguments)]
async fn run_review_step(
    review_cfg: &Option<Result<config::ReviewConfig, config::ConfigError>>,
    wf_map: &std::collections::HashMap<
        bridge_core::ids::WorkflowId,
        std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
    >,
    executor: &bridge_workflow::executor::WorkflowExecutor,
    task: &str,
    base_sha: &str,
    head_sha: &str,
    clone_cwd: &bridge_core::SessionCwd,
    task_id: &str,
    attempt: u32,
) -> (review::ReviewOutcome, String) {
    let rcfg = match review_cfg {
        None => return (review::ReviewOutcome::NotConfigured, String::new()),
        Some(Err(e)) => {
            eprintln!("[implement] review: config error: {e:?}");
            return (review::ReviewOutcome::ConfigError, String::new());
        }
        Some(Ok(c)) => c,
    };
    let Some(graph) = wf_map.get(&rcfg.workflow).cloned() else {
        return (review::ReviewOutcome::NotLoaded, String::new());
    };
    let input = review::build_review_input(task, base_sha, head_sha);
    let ctx = bridge_workflow::executor::WorkflowRunContext {
        session_cwd: Some(clone_cwd.clone()),
    };
    let token = tokio_util::sync::CancellationToken::new();
    let stream = executor.run_with_context(
        graph,
        input,
        format!("impl-review-{task_id}-{attempt}"),
        token.clone(),
        ctx,
    );
    eprintln!("[implement] review: running implement-review (attempt {attempt})");
    let mut drain = std::pin::pin!(drain_review(stream));
    let (completed, synth, reviewers_failed) = tokio::select! {
        r = &mut drain => r,
        _ = tokio::time::sleep(rcfg.timeout) => {
            eprintln!("[implement] review: timed out after {:?}", rcfg.timeout);
            token.cancel();
            (&mut drain).await
        }
    };
    if !completed {
        return (review::ReviewOutcome::Incomplete, String::new());
    }
    let (verdict, summary) = review::parse_verdict(&synth);
    if !matches!(verdict, review::Verdict::Approve) {
        eprintln!(
            "[implement] review {verdict:?}:\n{}",
            verify::truncate_output(&synth, rcfg.max_output_bytes)
        );
    }
    (
        review::ReviewOutcome::Ran {
            verdict,
            summary,
            reviewers_failed,
        },
        synth,
    )
}

/// The production `tweak::TweakEffects`: the real verify/review/fix turns. Borrows the loop's setup for its
/// lifetime; `fix` is only called when `fix_graph` is `Some` (the loop guards with `fix_available`).
struct ProdEffects<'a> {
    verify_cfg: &'a Option<Result<config::VerifyConfig, config::ConfigError>>,
    review_cfg: &'a Option<Result<config::ReviewConfig, config::ConfigError>>,
    wf_map: &'a std::collections::HashMap<
        bridge_core::ids::WorkflowId,
        std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
    >,
    executor: &'a bridge_workflow::executor::WorkflowExecutor,
    /// B2b-3c: the warm turn runner + its ONE stable session — fix turns continue it (not the executor).
    runner: &'a dyn turn::TurnRunner,
    impl_session: &'a bridge_core::ids::SessionId,
    /// The fix node's prompt template (rendered with `{{input}}`); `None` ⇒ fix is never called.
    fix_template: Option<String>,
    clone_cwd: &'a bridge_core::SessionCwd,
    repo: &'a std::path::Path,
    task: &'a str,
    base_sha: &'a str,
    task_id: &'a str,
}

#[async_trait::async_trait]
impl tweak::TweakEffects for ProdEffects<'_> {
    async fn verify(&mut self, _attempt: u32) -> verify::VerifyOutcome {
        run_verify_step(self.verify_cfg, self.clone_cwd, self.repo)
    }
    async fn review(&mut self, attempt: u32, head_sha: &str) -> (review::ReviewOutcome, String) {
        run_review_step(
            self.review_cfg,
            self.wf_map,
            self.executor,
            self.task,
            self.base_sha,
            head_sha,
            self.clone_cwd,
            self.task_id,
            attempt,
        )
        .await
    }
    async fn fix(&mut self, _attempt: u32, input: &str) -> bool {
        // Continue the SAME warm session — no new container, no new ACP session (the continuity).
        let template = self
            .fix_template
            .as_deref()
            .expect("fix only called when fix_available");
        let vars: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::from([("input", input)]);
        let parts = vec![bridge_core::domain::Part {
            text: bridge_workflow::template::render(template, &vars),
        }];
        self.runner.run_turn(self.impl_session, parts).await
    }
}

struct WarmImpl {
    warm: Arc<dyn bridge_core::ports::AgentBackend>,
    rebuild: Arc<dyn resilient::WarmRebuild>,
    impl_session: bridge_core::ids::SessionId,
    session_spec: bridge_core::domain::SessionSpec,
    edit_template: String,
    fix_template: Option<String>,
}

struct ContainerWarmRebuild {
    ccfg: bridge_container::ContainerRwConfig,
    spawn: Arc<dyn bridge_container::ContainerSpawn>,
    owner: String,
}

#[async_trait::async_trait]
impl resilient::WarmRebuild for ContainerWarmRebuild {
    async fn rebuild(&self) -> Result<Arc<dyn bridge_core::ports::AgentBackend>, BridgeError> {
        let warm = bridge_container::ContainerRwBackend::new_warm(
            self.ccfg.clone(),
            self.spawn.clone(),
            self.owner.clone(),
        )
        .await?;
        Ok(Arc::new(warm) as Arc<dyn bridge_core::ports::AgentBackend>)
    }
}

#[allow(clippy::too_many_arguments)]
async fn build_warm_impl(
    graph: &bridge_workflow::graph::WorkflowGraph,
    fix_graph: Option<&bridge_workflow::graph::WorkflowGraph>,
    snapshot: &bridge_core::domain::RegistrySnapshot,
    owner_config_path: &Path,
    run: &bridge_core::run_identity::RunHandle,
    policy: Arc<dyn PolicyEngine>,
    clone_cwd: &bridge_core::SessionCwd,
    task_id: &str,
    session_suffix: Option<&str>,
) -> Result<WarmImpl, BoxError> {
    let impl_entry =
        resolve_impl_identity(graph, fix_graph, snapshot).map_err(|e| format!("implement: {e}"))?;
    let edit_template = graph.nodes[0].prompt_template.clone();
    let fix_template = fix_graph.map(|g| g.nodes[0].prompt_template.clone());
    let ccfg = container_rw_cfg_from_entry(&impl_entry, run)?;
    let warm_owner = container_owner(
        owner_config_path,
        ccfg.sandbox.mount.as_str(),
        impl_entry.id.as_str(),
    );
    let cspawn =
        Arc::new(AcpContainerSpawn { policy }) as Arc<dyn bridge_container::ContainerSpawn>;
    let warm = bridge_container::ContainerRwBackend::new_warm(
        ccfg.clone(),
        cspawn.clone(),
        warm_owner.clone(),
    )
    .await?;
    let session_id = match session_suffix {
        Some(suffix) => format!("implement-{task_id}-{suffix}"),
        None => format!("implement-{task_id}"),
    };
    let impl_session = bridge_core::ids::SessionId::parse(session_id)
        .map_err(|e| format!("implement: session id: {e:?}"))?;
    let session_spec = bridge_core::domain::SessionSpec {
        config: bridge_core::domain::effective_config(&impl_entry, None),
        cwd: Some(clone_cwd.clone()),
    };
    warm.configure_session(&impl_session, &session_spec).await?;

    Ok(WarmImpl {
        warm: Arc::new(warm) as Arc<dyn bridge_core::ports::AgentBackend>,
        rebuild: Arc::new(ContainerWarmRebuild {
            ccfg,
            spawn: cspawn,
            owner: warm_owner,
        }),
        impl_session,
        session_spec,
        edit_template,
        fix_template,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_warm_loop(
    clone: &Path,
    repo: &Path,
    branch: &str,
    task: &str,
    base_sha: &str,
    task_id: &str,
    sha: String,
    original_message: &str,
    handoff_subject: &str,
    start_attempt: u32,
    max_attempts: u32,
    fix_available: bool,
    runner: &resilient::ResilientWarm,
    impl_session: &bridge_core::ids::SessionId,
    clone_cwd: &bridge_core::SessionCwd,
    verify_cfg: &Option<Result<config::VerifyConfig, config::ConfigError>>,
    review_cfg: &Option<Result<config::ReviewConfig, config::ConfigError>>,
    wf_map: &std::collections::HashMap<
        bridge_core::ids::WorkflowId,
        std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
    >,
    executor: &bridge_workflow::executor::WorkflowExecutor,
    fix_template: Option<String>,
    prod_ckpt: &mut implement_resume::ProdCheckpoint,
) -> implement_resume::ImplementPhase {
    let final_ = {
        let mut effects = ProdEffects {
            verify_cfg,
            review_cfg,
            wf_map,
            executor,
            runner,
            impl_session,
            fix_template,
            clone_cwd,
            repo,
            task,
            base_sha,
            task_id,
        };
        tweak::run_tweak_loop(
            clone,
            branch,
            task,
            sha,
            original_message,
            start_attempt,
            max_attempts,
            fix_available,
            &mut effects,
            prod_ckpt,
        )
        .await
    };

    let mut handoff = implement::handoff_text(
        &clone.to_string_lossy(),
        branch,
        &final_.sha,
        handoff_subject,
        &repo.to_string_lossy(),
    );
    handoff.push('\n');
    handoff.push_str(&verify::outcome_suffix(&final_.last_verify));
    handoff.push('\n');
    handoff.push_str(&review::outcome_suffix(&final_.last_review));
    handoff.push('\n');
    handoff.push_str(&tweak::loop_outcome_suffix(&final_.report));
    println!("{handoff}");

    let terminal = if final_.report.stop_reason == tweak::StopReason::Success {
        implement_resume::ImplementPhase::Approved
    } else {
        implement_resume::ImplementPhase::LoopStopped
    };
    implement_resume::write_terminal(clone, prod_ckpt.ck.clone(), terminal);
    let _ = runner.retire().await;
    terminal
}

/// `implement --merge` sugar (ADR-0027): when the run ended `Approved`, land it via `merge_clone`
/// (Approved-only, `--force` n/a — `implement` has no `--force`) and exit with its code; a non-Approved
/// run prints `not merged:` and exits 2. Without `--merge`, returns `Ok(())` (plain implement unchanged).
fn merge_after_loop(
    merge_requested: bool,
    outcome_phase: implement_resume::ImplementPhase,
    merge_cfg: Option<Result<config::MergeConfig, config::ConfigError>>,
    clone: &Path,
    root: &Path,
    onto: Option<&str>,
) -> Result<(), BoxError> {
    if !merge_requested {
        return Ok(());
    }
    match outcome_phase {
        implement_resume::ImplementPhase::Approved => {
            let mcfg = merge_cfg
                .transpose()
                .map_err(|e| format!("implement --merge: {e}"))?;
            let outcome = merge::merge_clone(mcfg.as_ref(), clone, root, onto, false);
            use std::io::Write;
            std::io::stdout().flush().ok();
            std::process::exit(outcome.code());
        }
        other => {
            eprintln!("not merged: run ended {other:?}, not Approved — resume/re-run the agent");
            std::process::exit(2);
        }
    }
}

/// `a2a-bridge implement <task> --repo <path>` — clone a quarantine, run the 1-node `implement-edit`
/// workflow on the ContainerRw `impl` agent (session_cwd = the clone), then commit + the bounded
/// review→tweak loop (B2b-3b) + the operator hand-off. The agent owns staging; the bridge owns the commit.
async fn implement_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{IMPLEMENT_USAGE}");
        return Ok(());
    }
    bridge_observ::init();
    let a = parse_implement_args(args)?;
    let config_path = a.config.clone();
    let merge_requested = a.merge;
    let onto = a.onto.clone();
    let (task, repo, base_ref, workflow) = match a.mode {
        ImplementMode::Fresh {
            task,
            repo,
            base_ref,
            workflow,
        } => (task, repo, base_ref, workflow),
        ImplementMode::Resume { resume_id } => {
            return implement_resume_cmd(
                &resume_id,
                &config_path,
                merge_requested,
                onto.as_deref(),
            )
            .await;
        }
    };

    // 1. config + canonical allowed_cwd_root (the ContainerRw mount anchor).
    let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("implement: read config {:?}: {e}", config_path))?;
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

    // B2b-3b: resolve the loop config PRE-CLONE so a malformed [implement] fails loud before any quarantine
    // clone is created. Absent → LoopConfig::default() (loop ON, max_attempts=3). `fix_graph` is resolved
    // later (it needs the loaded workflow map).
    let loop_cfg = cfg
        .implement
        .as_ref()
        .map(|t| t.to_config())
        .transpose()
        .map_err(|e| format!("implement: [implement] config: {e}"))?
        .unwrap_or_default();

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
    let refname = base_ref.clone().unwrap_or_else(|| "HEAD".into());
    let rp = implement::run_git(Some(&repo), &["rev-parse", &refname])
        .map_err(|e| format!("implement: rev-parse {refname:?} in {:?}: {e}", repo))?;
    if !rp.status.success() {
        return Err(format!(
            "implement: base-ref {refname:?}: {}",
            String::from_utf8_lossy(&rp.stderr).trim()
        )
        .into());
    }
    let base_sha = String::from_utf8_lossy(&rp.stdout).trim().to_string();

    // 3. clone (committed-only) + checkout the base SHA + the task branch.
    implement::do_clone(&repo.to_string_lossy(), &clone.to_string_lossy())?;
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
    let base = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    let wf_map = cfg
        .load_workflows(&base)
        .map_err(|e| format!("implement: workflow load: {e}"))?;
    let wf_id = bridge_core::ids::WorkflowId::parse(workflow.clone())
        .map_err(|e| format!("implement: workflow id {:?}: {e:?}", workflow))?;
    let graph = wf_map
        .get(&wf_id)
        .cloned()
        .ok_or_else(|| format!("implement: unknown workflow {:?}", workflow))?;
    // B2b-3b: resolve the fix workflow against the loaded map (None → FixUnavailable, a soft loop stop).
    let fix_graph = wf_map.get(&loop_cfg.fix_workflow).cloned();
    // B2b-2: parse [verify] NOW, before into_snapshot moves cfg. Owned + Clone, survives the move.
    let verify_cfg = cfg.verify.as_ref().map(|t| t.to_config());
    let review_cfg = cfg.review.as_ref().map(|t| t.to_config()); // B2b-3a: parsed pre-commit (beside verify)
    let merge_cfg = cfg.merge.as_ref().map(|m| m.to_config()); // ADR-0027: parsed pre-move (--merge sugar)
    let snapshot = cfg
        .into_snapshot()
        .map_err(|e| format!("implement: snapshot: {e}"))?;
    // Gate the [verify] runtime against the resolved allowlist: a non-allowlisted runtime rejects into
    // VerifyOutcome::ConfigError (verify never runs on the wrong engine). snapshot.allowed_cmds is the
    // verbatim resolved list (no duplicated union to drift). verify_cfg is owned, so this shadows it.
    let verify_cfg = config::gate_verify_runtime(verify_cfg, &snapshot.allowed_cmds);
    preflight_runtimes(
        &snapshot,
        verify_cfg
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(|v| v.runtime.as_deref()),
    );
    // Canonical config path: the owner token must match between the sweeps and the spawn factory.
    let owner_config_path =
        std::fs::canonicalize(&config_path).unwrap_or_else(|_| config_path.clone());
    // Increment A: ONE run identity + its flock lease for the whole command. The held lease IS the
    // liveness signal a concurrent/later run's `classify_sweep` reads (lock free ⇒ this run died). Mint
    // BEFORE recovery so our own lease is held while we sweep crash-orphans (we never self-classify Dead).
    let host = bridge_core::liveness::host_id();
    let instance_id = format!("{}-{}", std::process::id(), implement::nonce(8));
    let _lease = bridge_core::liveness::acquire_lease(&instance_id)
        .map_err(|e| format!("implement: acquire run lease: {e}"))?;
    let run = bridge_core::run_identity::RunHandle {
        instance_id: instance_id.clone(),
        host: host.clone(),
        lease: _lease.path().to_string_lossy().to_string(),
        start: epoch_secs(),
    };
    // Before-first-use crash recovery: reap only DEAD (same host + free lease) orphans of THIS process's
    // owners (`:rw` ∪ `:ro`); a live concurrent run's lease is held → its containers are spared.
    recover_orphans(&snapshot, &owner_config_path, &host);
    // Label-scoped END-sweep backstop (THIS run's `a2a.run` only). Declared BEFORE `warm` → drops AFTER it
    // (the warm `retire` reaps first; this catches anything it missed, including the `:ro` reviewers).
    let _run_guard = RunEndGuard {
        runtimes: run_guard_runtimes(&snapshot, &owner_config_path),
        instance_id: instance_id.clone(),
    };
    let policy: Arc<dyn PolicyEngine> = Arc::new(AutoPolicy);

    // B2b-3c: build the WARM :rw backend for the impl agent BEFORE the snapshot / owner_config_path moves
    // below. The edit + fix turns run on this ONE container + ONE ACP session (off the executor); review
    // still uses the registry/executor built afterward. All reads of `snapshot`/`owner_config_path` happen
    // here, before the moves.
    let warm_impl = build_warm_impl(
        &graph,
        fix_graph.as_deref(),
        &snapshot,
        &owner_config_path,
        &run,
        Arc::clone(&policy),
        &clone_cwd,
        &task_id,
        None,
    )
    .await?;
    let warm_runner = Arc::new(resilient::ResilientWarm::new(
        warm_impl.warm.clone(),
        warm_impl.rebuild.clone(),
        warm_impl.session_spec.clone(),
        loop_cfg.max_session_respawns,
        {
            let clone = clone.clone();
            Arc::new(move || implement::reset_worktree_to_head(&clone))
        },
    ));

    // The registry + executor are still built — REVIEW runs through them (edit/fix are off-executor).
    let spawn = make_spawn_fn(Arc::clone(&policy), owner_config_path, run);
    let registry = Arc::new(
        bridge_registry::registry::Registry::new(snapshot, spawn)
            .map_err(|e| format!("implement: registry: {e:?}"))?,
    );
    let executor = bridge_workflow::executor::WorkflowExecutor::new(
        Arc::clone(&registry) as Arc<dyn bridge_core::ports::AgentRegistry>
    );

    // First edit turn — on the WARM session (off the executor). Render the single-node `{{input}}` template.
    let edit_vars: std::collections::HashMap<&str, &str> =
        std::collections::HashMap::from([("input", task.as_str())]);
    let edit_input = bridge_workflow::template::render(&warm_impl.edit_template, &edit_vars);
    let completed = turn::TurnRunner::run_turn(
        warm_runner.as_ref(),
        &warm_impl.impl_session,
        vec![bridge_core::domain::Part { text: edit_input }],
    )
    .await;

    // 5. the pure soft-gate decision, then execute its Action. A container exists now, so any post-edit
    // early-return must `retire()` (the RunEndGuard is only the backstop).
    let guard = implement::head_guard(&clone, &branch, &pre);
    let stage = match implement::stage_state(&clone) {
        Ok(s) => s,
        Err(e) => {
            let _ = warm_runner.retire().await;
            return Err(format!("implement: stage check: {e}").into());
        }
    };
    let msg = implement::commit_message(implement::read_commit_msg_file(&clone), &task);
    if msg.1 {
        eprintln!("[implement] no .git/A2A_COMMIT_MSG — using task-derived message");
    }
    match implement::decide(completed, guard, stage, msg) {
        implement::Action::Abort(reason) => {
            eprintln!(
                "[implement] {reason} — NO commit; clone left at {}",
                clone.display()
            );
            let _ = warm_runner.retire().await; // sole warm reap site; RunEndGuard is the backstop
            Err(format!("implement: {reason}").into())
        }
        implement::Action::NoCommitClean => {
            println!(
                "implement: made no changes; clone left at {}",
                clone.display()
            );
            let _ = warm_runner.retire().await;
            Ok(())
        }
        implement::Action::NoCommitDirty => {
            eprintln!(
                "[implement] agent edited but staged NOTHING — NOT committing (agent owns staging). \
                 Clone left at {} for inspection.",
                clone.display()
            );
            let _ = warm_runner.retire().await;
            Ok(())
        }
        implement::Action::Commit(message) => {
            // Phase 1 → 2 boundary: the LAST fallible setup step. Retire the warm container on its error
            // path too (a container exists); after this the loop body is lossy (no `?`).
            let sha = match implement::host_commit(&clone, &message) {
                Ok(s) => s,
                Err(e) => {
                    let _ = warm_runner.retire().await;
                    return Err(e.into());
                }
            };
            let _ = std::fs::remove_file(clone.join(".git").join("A2A_COMMIT_MSG")); // R13 hygiene
                                                                                     // ADR-0026: build the resume checkpoint (FirstCommitCreated) before the loop.
            let mut prod_ckpt = implement_resume::ProdCheckpoint {
                clone: clone.clone(),
                ck: implement_resume::ImplementCheckpoint {
                    schema_version: implement_resume::SCHEMA_VERSION,
                    resume_id: task_id.clone(),
                    task_id: task_id.clone(),
                    task_brief: task.clone(),
                    source_repo: repo.clone(),
                    clone_path: clone.clone(),
                    config_path: config_path.clone(),
                    branch: branch.clone(),
                    base_ref: base_ref.clone(),
                    base_commit: base_sha.clone(),
                    current_commit: Some(sha.clone()),
                    original_message: Some(message.clone()),
                    edit_workflow: workflow.clone(),
                    fix_workflow: loop_cfg.fix_workflow.as_str().to_string(),
                    loop_max_attempts: loop_cfg.max_attempts,
                    attempt_next: 1,
                    phase: implement_resume::ImplementPhase::FirstCommitCreated,
                    created_at_ms: implement_resume::now_ms(),
                    updated_at_ms: implement_resume::now_ms(),
                },
            };
            let _ = implement_resume::save_checkpoint(&clone, &prod_ckpt.ck);
            let subject = message.lines().next().unwrap_or("").to_string();
            let WarmImpl {
                impl_session,
                fix_template,
                ..
            } = warm_impl;
            let outcome_phase = run_warm_loop(
                &clone,
                &repo,
                &branch,
                &task,
                &base_sha,
                &task_id,
                sha,
                &message,
                &subject,
                1, // start_attempt (fresh run)
                loop_cfg.max_attempts,
                fix_graph.is_some(),
                warm_runner.as_ref(),
                &impl_session,
                &clone_cwd,
                &verify_cfg,
                &review_cfg,
                &wf_map,
                &executor,
                fix_template,
                &mut prod_ckpt,
            )
            .await;
            merge_after_loop(
                merge_requested,
                outcome_phase,
                merge_cfg,
                &clone,
                &root,
                onto.as_deref(),
            )
        }
    }
}

async fn implement_resume_cmd(
    resume_id: &str,
    config_path: &Path,
    merge_requested: bool,
    onto: Option<&str>,
) -> Result<(), BoxError> {
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("implement --resume: read config {config_path:?}: {e}"))?;
    let cfg = config::RegistryConfig::parse(&raw)
        .map_err(|e| format!("implement --resume: config parse: {e}"))?;
    let loop_cfg = cfg
        .implement
        .as_ref()
        .map(|t| t.to_config())
        .transpose()
        .map_err(|e| format!("implement --resume: [implement] config: {e}"))?
        .unwrap_or_default();
    let root = cfg
        .allowed_cwd_root
        .clone()
        .ok_or("implement --resume: config needs allowed_cwd_root")?;
    let root = std::fs::canonicalize(&root)
        .map_err(|e| format!("implement --resume: allowed_cwd_root {root:?}: {e}"))?;
    let clone = implement_resume::resolve_clone(&root, resume_id)?;
    let ck = implement_resume::load_checkpoint(&clone)?;
    implement_resume::validate_resumable(&ck)?;

    let lock_dir = clone.join(".git").join("a2a-bridge").join("locks");
    std::fs::create_dir_all(&lock_dir)
        .map_err(|e| format!("implement --resume: mkdir {lock_dir:?}: {e}"))?;
    let _takeover = bridge_core::liveness::acquire_lease_in(&lock_dir, "implement-resume")
        .map_err(|e| format!("implement --resume: another resume holds {resume_id} ({e})"))?;

    let resume_sha = implement_resume::reconcile_head(&clone, &ck)?;
    let clone_cwd = bridge_core::SessionCwd::parse(&clone.to_string_lossy())?;

    let base = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let wf_map = cfg
        .load_workflows(&base)
        .map_err(|e| format!("implement --resume: workflow load: {e}"))?;
    let wf_id = bridge_core::ids::WorkflowId::parse(ck.edit_workflow.clone()).map_err(|e| {
        format!(
            "implement --resume: workflow id {:?}: {e:?}",
            ck.edit_workflow
        )
    })?;
    let graph = wf_map.get(&wf_id).cloned().ok_or_else(|| {
        format!(
            "implement --resume: unknown workflow {:?}",
            ck.edit_workflow
        )
    })?;
    let fix_wf_id = bridge_core::ids::WorkflowId::parse(ck.fix_workflow.clone()).map_err(|e| {
        format!(
            "implement --resume: fix workflow id {:?}: {e:?}",
            ck.fix_workflow
        )
    })?;
    let fix_graph = wf_map.get(&fix_wf_id).cloned();
    let verify_cfg = cfg.verify.as_ref().map(|t| t.to_config());
    let review_cfg = cfg.review.as_ref().map(|t| t.to_config());
    let merge_cfg = cfg.merge.as_ref().map(|m| m.to_config()); // ADR-0027: parsed pre-move (--merge sugar)
    let snapshot = cfg
        .into_snapshot()
        .map_err(|e| format!("implement --resume: snapshot: {e}"))?;
    // Same verify-runtime gate as the implement path (reject a disallowed runtime into ConfigError).
    let verify_cfg = config::gate_verify_runtime(verify_cfg, &snapshot.allowed_cmds);
    preflight_runtimes(
        &snapshot,
        verify_cfg
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(|v| v.runtime.as_deref()),
    );

    let owner_config_path =
        std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    let host = bridge_core::liveness::host_id();
    let instance_id = format!("{}-{}", std::process::id(), implement::nonce(8));
    let _lease = bridge_core::liveness::acquire_lease(&instance_id)
        .map_err(|e| format!("implement --resume: acquire run lease: {e}"))?;
    let run = bridge_core::run_identity::RunHandle {
        instance_id: instance_id.clone(),
        host: host.clone(),
        lease: _lease.path().to_string_lossy().to_string(),
        start: epoch_secs(),
    };
    recover_orphans(&snapshot, &owner_config_path, &host);
    let _run_guard = RunEndGuard {
        runtimes: run_guard_runtimes(&snapshot, &owner_config_path),
        instance_id: instance_id.clone(),
    };

    let policy: Arc<dyn PolicyEngine> = Arc::new(AutoPolicy);
    let suffix = format!("r{}", implement::nonce(6));
    let warm_impl = build_warm_impl(
        &graph,
        fix_graph.as_deref(),
        &snapshot,
        &owner_config_path,
        &run,
        Arc::clone(&policy),
        &clone_cwd,
        &ck.task_id,
        Some(&suffix),
    )
    .await?;
    let warm_runner = Arc::new(resilient::ResilientWarm::new(
        warm_impl.warm.clone(),
        warm_impl.rebuild.clone(),
        warm_impl.session_spec.clone(),
        loop_cfg.max_session_respawns,
        {
            let clone = clone.clone();
            Arc::new(move || implement::reset_worktree_to_head(&clone))
        },
    ));

    let spawn = make_spawn_fn(Arc::clone(&policy), owner_config_path, run);
    let registry = Arc::new(
        bridge_registry::registry::Registry::new(snapshot, spawn)
            .map_err(|e| format!("implement --resume: registry: {e:?}"))?,
    );
    let executor = bridge_workflow::executor::WorkflowExecutor::new(
        Arc::clone(&registry) as Arc<dyn bridge_core::ports::AgentRegistry>
    );

    let mut prod_ckpt = implement_resume::ProdCheckpoint {
        clone: clone.clone(),
        ck: ck.clone(),
    };
    let subject = implement::commit_subject(&clone).unwrap_or_else(|_| {
        ck.original_message
            .as_deref()
            .and_then(|m| m.lines().next())
            .unwrap_or("")
            .to_string()
    });
    let original_message = ck
        .original_message
        .clone()
        .unwrap_or_else(|| subject.clone());
    let task = format!(
        "{}\n\nPrior session state is unavailable; the repository tree and committed diff are authoritative.",
        ck.task_brief
    );
    let WarmImpl {
        impl_session,
        fix_template,
        ..
    } = warm_impl;
    let outcome_phase = run_warm_loop(
        &clone,
        &ck.source_repo,
        &ck.branch,
        &task,
        &ck.base_commit,
        &ck.task_id,
        resume_sha,
        &original_message,
        &subject,
        ck.attempt_next,
        ck.loop_max_attempts,
        fix_graph.is_some(),
        warm_runner.as_ref(),
        &impl_session,
        &clone_cwd,
        &verify_cfg,
        &review_cfg,
        &wf_map,
        &executor,
        fix_template,
        &mut prod_ckpt,
    )
    .await;
    merge_after_loop(
        merge_requested,
        outcome_phase,
        merge_cfg,
        &clone,
        &root,
        onto,
    )
}

/// Execute the `run-workflow` subcommand.
/// Loads the config, resolves the workflow graph, runs the executor,
/// prints NodeStarted/NodeFinished to stderr and the terminal output to stdout
/// (or `--out <file>`).
async fn run_workflow_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{RUN_WORKFLOW_USAGE}");
        return Ok(());
    }
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
    let mut snapshot = cfg
        .into_snapshot()
        .map_err(|e| format!("run-workflow: registry snapshot error: {e}"))?;
    preflight_runtimes(&snapshot, None); // warn early if a sandbox runtime isn't up (verify is implement-only)
                                         // MAJOR 4 (ADR-0028): stamp --session-cwd into every entry's `session_cwd` so the ONE resolution
                                         // chain (`resolve_static_session_cwd`) feeds BOTH the agent's ACP session cwd AND the codex native
                                         // MCP `-c` args' `{cwd}` from the same value — prism then indexes exactly the repo the agent works
                                         // in (no silent wrong-repo). Acp-kind agents read this at spawn; container_rw gets the per-turn cwd
                                         // via the run context instead. run-workflow targets one repo, so stamping all entries is correct.
    if let Some(ref dir) = session_cwd {
        for e in &mut snapshot.entries {
            e.session_cwd = Some(dir.clone());
        }
    }
    // Canonical config path: the owner token must match between the sweeps and the spawn factory.
    let owner_config_path =
        std::fs::canonicalize(&config_path).unwrap_or_else(|_| config_path.clone());
    // Increment A: one run identity + flock lease for the command; before-first-use crash recovery + a
    // label-scoped END-sweep (THIS run only — concurrent runs are untouched).
    let host = bridge_core::liveness::host_id();
    let instance_id = format!("{}-{}", std::process::id(), implement::nonce(8));
    let _lease = bridge_core::liveness::acquire_lease(&instance_id)
        .map_err(|e| format!("run-workflow: acquire run lease: {e}"))?;
    let run = bridge_core::run_identity::RunHandle {
        instance_id: instance_id.clone(),
        host: host.clone(),
        lease: _lease.path().to_string_lossy().to_string(),
        start: epoch_secs(),
    };
    recover_orphans(&snapshot, &owner_config_path, &host);
    let _run_guard = RunEndGuard {
        runtimes: run_guard_runtimes(&snapshot, &owner_config_path),
        instance_id: instance_id.clone(),
    };
    let policy = Arc::new(bridge_policy::permission::AutoPolicy);
    let policy_for_spawn = Arc::clone(&policy) as Arc<dyn bridge_core::ports::PolicyEngine>;
    let spawn = make_spawn_fn(policy_for_spawn, owner_config_path, run);
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
        "kiro" => {
            "\n# kiro: zero-auth local default (kiro-cli acp). Unpinned (default model `auto`).\n# kiro advertises its model via the `models` surface + session/set_model, so you MAY\n# pin an advertised id, e.g. model = \"claude-sonnet-4.5\".\n[[agents]]\nid   = \"kiro\"\ncmd  = \"kiro-cli\"\nargs = [\"acp\"]\n"
        }
        "codex" => {
            "\n# codex: gpt-5.5 with reasoning_effort.\n[[agents]]\nid    = \"codex\"\ncmd   = \"codex-acp\"\nmodel = \"gpt-5.5\"\neffort = \"high\"\n"
        }
        "claude" => {
            "\n# claude: subscription. `model` is validated against the advertised values and\n# applied; aliases work too (e.g. model = \"fable\" -> claude-fable-5[1m]).\n[[agents]]\nid    = \"claude\"\ncmd   = \"claude-agent-acp\"\nmodel = \"sonnet\"\n"
        }
        "api" => {
            "\n# api: OpenAI-compatible non-process backend. `api_key_env` is the NAME of an\n# env var holding the token (never the secret itself). Effort is not applied for api.\n[[agents]]\nid          = \"api\"\nkind        = \"api\"\nbase_url    = \"https://api.openai.com/v1\"\napi_key_env = \"OPENAI_API_KEY\"\nmodel       = \"gpt-4o-mini\"\n"
        }
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
    let cfg_path = dir.join("a2a-bridge.toml").display().to_string();
    println!("Serve:         a2a-bridge serve --config {cfg_path}");
    println!(
        "Run a workflow: a2a-bridge run-workflow code-review --input <file> --session-cwd <repo> --config {cfg_path}"
    );
    println!(
        "More:          a2a-bridge help  (and `a2a-bridge <subcommand> --help`); quickstart in AGENTS.md"
    );
    Ok(())
}

/// Execute `a2a-bridge containers <list|reap>` — the operator view/cleanup over Increment A's managed
/// containers. Reads the docker labels + probes the per-run `flock` lease to classify each container
/// (Alive/Dead/Unknown), scoped (by default) to the owners THIS `--config` would spawn. Pure cores live in
/// the `containers` module; this orchestrates the config load + docker shell-out + lease probe. Synchronous
/// (no async I/O — just `std::process` + filesystem lease probes).
fn containers_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", containers::CONTAINERS_USAGE);
        return Ok(());
    }
    // Subcommand: list (default) | reap (a leading non-flag token).
    let has_sub = args.first().map(|a| !a.starts_with("--")).unwrap_or(false);
    let sub = if has_sub { args[0].as_str() } else { "list" };

    let mut config: Option<PathBuf> = None;
    let mut all = false;
    let mut older_than: Option<String> = None;
    let mut flags = containers::ReapFlags::default();
    let mut it = args.iter();
    if has_sub {
        it.next(); // consume the subcommand positional
    }
    while let Some(f) = it.next() {
        match f.as_str() {
            "--config" => {
                config = Some(PathBuf::from(
                    it.next().ok_or("containers: --config needs a value")?,
                ))
            }
            "--all" => all = true,
            "--all-dead" => flags.all_dead = true,
            "--stale" => flags.stale = true,
            "--older-than" => {
                older_than = Some(
                    it.next()
                        .ok_or("containers: --older-than needs a value")?
                        .clone(),
                )
            }
            "--run" => {
                flags.run = Some(it.next().ok_or("containers: --run needs a value")?.clone())
            }
            "--owner" => {
                flags.owner = Some(
                    it.next()
                        .ok_or("containers: --owner needs a value")?
                        .clone(),
                )
            }
            "--force" => {
                flags.force = Some(
                    it.next()
                        .ok_or("containers: --force needs a value")?
                        .clone(),
                )
            }
            other => {
                return Err(format!(
                    "containers: unknown flag {other:?}\n{}",
                    containers::CONTAINERS_USAGE
                )
                .into());
            }
        }
    }
    let window = older_than.as_deref().unwrap_or("1h");
    let config_path = config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH));

    // Derive THIS config's owners + runtimes (the default scope), exactly as the spawn factory does.
    let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("containers: read config {config_path:?}: {e}"))?;
    let cfg = config::RegistryConfig::parse(&raw)
        .map_err(|e| format!("containers: config parse: {e}"))?;
    let snapshot = cfg
        .into_snapshot()
        .map_err(|e| format!("containers: snapshot: {e}"))?;
    let owner_config_path =
        std::fs::canonicalize(&config_path).unwrap_or_else(|_| config_path.clone());
    let mut my_owners: Vec<String> = rw_sweep_targets(&snapshot, &owner_config_path)
        .into_iter()
        .chain(ro_sweep_targets(&snapshot, &owner_config_path))
        .map(|(_runtime, owner)| owner)
        .collect();
    my_owners.sort();
    my_owners.dedup();
    let runtimes = {
        let r = run_guard_runtimes(&snapshot, &owner_config_path);
        if r.is_empty() {
            vec!["docker".to_string()]
        } else {
            r
        }
    };
    let my_host = bridge_core::liveness::host_id();
    let probe = bridge_core::liveness::FsLeaseProbe;

    // Read + classify every managed container across this config's runtimes (a record's host+lease drive
    // the liveness verdict; `is_stale` is only meaningful — and only probed — for Alive ones).
    let mut classified: Vec<containers::ClassifiedRecord> = Vec::new();
    let mut managed_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for runtime in &runtimes {
        let Ok(out) = std::process::Command::new(runtime)
            .args([
                "ps",
                "-a",
                "--filter",
                "label=a2a.managed=1",
                "--format",
                containers::LIST_FORMAT,
            ])
            .output()
        else {
            continue;
        };
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Some(rec) = containers::parse_record(line) else {
                continue;
            };
            managed_names.insert(rec.name.clone());
            let labels = std::collections::HashMap::from([
                ("a2a.host".to_string(), rec.host.clone()),
                ("a2a.lease".to_string(), rec.lease.clone()),
            ]);
            let verdict = bridge_core::run_identity::classify(&labels, &my_host, &probe);
            let stale = verdict == bridge_core::run_identity::Verdict::Alive
                && bridge_core::reaper::is_stale(runtime, &rec.name, window);
            classified.push(containers::ClassifiedRecord {
                rec,
                verdict,
                stale,
            });
        }
    }

    match sub {
        "list" => {
            let now = epoch_secs().parse::<u64>().unwrap_or(0);
            println!("{}", containers::LIST_HEADER);
            let mut shown = 0;
            for c in &classified {
                if containers::list_visible(&c.rec, all, &my_owners) {
                    println!("{}", containers::format_row(c, now));
                    shown += 1;
                }
            }
            if shown == 0 {
                println!(
                    "(no managed containers{})",
                    if all {
                        ""
                    } else {
                        " for this config — pass --all to show every owner"
                    }
                );
            }
            // Legacy pass: pre-Increment-A unlabeled `a2a-(ro|rw)-*` names (no `a2a.managed`), list-only.
            let mut legacy: Vec<String> = Vec::new();
            for runtime in &runtimes {
                if let Ok(out) = std::process::Command::new(runtime)
                    .args(["ps", "-a", "--format", "{{.Names}}"])
                    .output()
                {
                    for name in String::from_utf8_lossy(&out.stdout).lines() {
                        let name = name.trim();
                        if containers::is_legacy_name(name) && !managed_names.contains(name) {
                            legacy.push(name.to_string());
                        }
                    }
                }
            }
            legacy.sort();
            legacy.dedup();
            for name in legacy {
                println!(
                    "{name:<28} -    legacy   -          -        -      -     (reap with --force {name})"
                );
            }
            Ok(())
        }
        "reap" => {
            let plan = containers::reap_plan(&classified, &flags, &my_owners);
            if plan.is_empty() {
                println!("containers reap: nothing to reap");
                return Ok(());
            }
            for name in &plan {
                // Reap on every runtime (idempotent — `rm -f` of a gone/absent name is harmless).
                for runtime in &runtimes {
                    let _ = std::process::Command::new(runtime)
                        .args(["rm", "-f", name])
                        .output();
                }
                println!("reaped {name}");
            }
            Ok(())
        }
        other => Err(format!(
            "containers: unknown action {other:?} (expected: list | reap)\n{}",
            containers::CONTAINERS_USAGE
        )
        .into()),
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    // Dispatch subcommands BEFORE the server path touches the filesystem.
    let raw_args: Vec<String> = std::env::args().collect();
    match raw_args.get(1).map(|s| s.as_str()) {
        Some("run-workflow") => return run_workflow_cmd(&raw_args[2..]).await,
        Some("implement") => return implement_cmd(&raw_args[2..]).await,
        Some("merge") => return merge::merge_cmd(&raw_args[2..]).await,
        Some("containers") => return containers_cmd(&raw_args[2..]),
        Some("submit") => return submit_cmd(&raw_args[2..]).await,
        Some("task") => return task_cmd(&raw_args[2..]).await,
        Some("init") => return init_cmd(&raw_args[2..]),
        Some("help") | Some("--help") | Some("-h") => {
            println!("{TOP_USAGE}");
            return Ok(());
        }
        // `serve` (explicit) and the bare invocation fall through to the server path.
        Some("serve") | None => {}
        // An unknown first token must NOT silently serve (a typo'd subcommand or flag
        // would otherwise be swallowed and the default served).
        Some(other) => {
            return Err(format!(
                "a2a-bridge: unknown subcommand {other:?} (expected: serve | run-workflow | implement | merge | containers | submit | task | init | help)"
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

    // Increment A: ONE run identity + flock lease for the serve lifetime (the lease — held until this
    // process exits — is what a future run's `classify_sweep` reads to decide we're alive). serve is
    // long-running with NO END-sweep: per-backend `retire` reaps with the runtime alive, and the next
    // run's before-first-use recovery catches anything a crash leaves behind.
    let host = bridge_core::liveness::host_id();
    let instance_id = format!("{}-{}", std::process::id(), implement::nonce(8));
    let _lease = bridge_core::liveness::acquire_lease(&instance_id)
        .map_err(|e| format!("serve: acquire run lease: {e}"))?;
    let run = bridge_core::run_identity::RunHandle {
        instance_id,
        host: host.clone(),
        lease: _lease.path().to_string_lossy().to_string(),
        start: epoch_secs(),
    };

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
    let run_for_spawn = run.clone();
    let spawn: SpawnFn = Arc::new(move |entry: Arc<AgentEntry>| {
        let policy = Arc::clone(&policy_for_spawn);
        let owner_config_path = owner_config_path.clone();
        let run = run_for_spawn.clone();
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
                    let (program, argv, acp) =
                        acp_spawn_inputs(&entry, cwd, &owner_config_path, &run)?;
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
                    let owner = {
                        let sb = entry.sandbox.as_ref().ok_or(BridgeError::ConfigInvalid {
                            reason: format!(
                                "container_rw agent {} requires sandbox",
                                entry.id.as_str()
                            ),
                        })?;
                        container_owner(&owner_config_path, &sb.mount, entry.id.as_str())
                    };
                    let ccfg = container_rw_cfg_from_entry(&entry, &run)?;
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
    preflight_runtimes(&snapshot, None); // warn early if a sandbox runtime isn't responding at serve boot
                                         // Increment A before-first-use crash recovery: reap only DEAD
                                         // (same host + free lease) orphans of this instance's owners
                                         // (`:rw` ∪ `:ro`); a live concurrent run holds its lease and is
                                         // spared. serve is long-running with no END-sweep — per-backend
                                         // retire reaps with the runtime alive, and the next run's recovery
                                         // catches any crash leftover.
    recover_orphans(&snapshot, &config_path, &host);
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
        let recover_config = config_path.clone();
        let recover_host = host.clone();
        tokio::spawn(async move {
            use futures::StreamExt;
            while let Some(snap) = watch.next().await {
                // Increment A: recover crash-orphans for any owners a hot-reload introduces, BEFORE
                // applying (idempotent; only DEAD same-host orphans are reaped, never a live run's).
                recover_orphans(&snap, &recover_config, &recover_host);
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
    use crate::turn::TurnRunner;

    #[tokio::test]
    async fn drain_turn_outcomes() {
        use bridge_core::ports::{BackendStream, Update};
        let done = |sr: &str| {
            Ok(Update::Done {
                stop_reason: sr.into(),
            })
        };
        // end_turn → complete
        let s: BackendStream = Box::pin(tokio_stream::iter(vec![done("end_turn")]));
        assert_eq!(
            turn::drain_turn(s).await,
            turn::TurnOutcome {
                completed: true,
                last_err: None,
            }
        );
        // cancelled → incomplete
        let s: BackendStream = Box::pin(tokio_stream::iter(vec![done("cancelled")]));
        assert_eq!(
            turn::drain_turn(s).await,
            turn::TurnOutcome {
                completed: false,
                last_err: None,
            }
        );
        // clean end without Done → incomplete (the executor-divergence guard)
        let s: BackendStream = Box::pin(tokio_stream::iter(Vec::<
            Result<Update, bridge_core::error::BridgeError>,
        >::new()));
        assert_eq!(
            turn::drain_turn(s).await,
            turn::TurnOutcome {
                completed: false,
                last_err: None,
            }
        );
        // stream error → incomplete, with the last pre-completion error preserved
        let crashed = bridge_core::error::BridgeError::agent_crashed("x");
        let s: BackendStream = Box::pin(tokio_stream::iter(vec![Err(crashed.clone())]));
        assert_eq!(
            turn::drain_turn(s).await,
            turn::TurnOutcome {
                completed: false,
                last_err: Some(crashed),
            }
        );
        // multiple pre-completion errors → the last one wins
        let first = bridge_core::error::BridgeError::agent_crashed("first");
        let last = bridge_core::error::BridgeError::AgentOverloaded;
        let s: BackendStream = Box::pin(tokio_stream::iter(vec![
            Err(first),
            Err(last.clone()),
            done("cancelled"),
        ]));
        assert_eq!(
            turn::drain_turn(s).await,
            turn::TurnOutcome {
                completed: false,
                last_err: Some(last),
            }
        );
        // Done then a trailing teardown Err → STILL complete (completion latches)
        let s: BackendStream = Box::pin(tokio_stream::iter(vec![
            done("end_turn"),
            Err(bridge_core::error::BridgeError::agent_crashed("teardown")),
        ]));
        assert_eq!(
            turn::drain_turn(s).await,
            turn::TurnOutcome {
                completed: true,
                last_err: None,
            }
        );
        // Pre-completion error remains observable even if the turn later completes.
        let pre_done = bridge_core::error::BridgeError::agent_crashed("before-done");
        let s: BackendStream = Box::pin(tokio_stream::iter(vec![
            Err(pre_done.clone()),
            done("end_turn"),
            Err(bridge_core::error::BridgeError::agent_crashed("teardown")),
        ]));
        assert_eq!(
            turn::drain_turn(s).await,
            turn::TurnOutcome {
                completed: true,
                last_err: Some(pre_done),
            }
        );
    }

    struct FakeTurnRunner {
        completed: bool,
        seen: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    }

    #[async_trait::async_trait]
    impl turn::TurnRunner for FakeTurnRunner {
        async fn run_turn(
            &self,
            session: &bridge_core::ids::SessionId,
            parts: Vec<bridge_core::domain::Part>,
        ) -> bool {
            self.seen.lock().unwrap().push((
                session.as_str().to_string(),
                parts.into_iter().map(|p| p.text).collect(),
            ));
            self.completed
        }
    }

    #[tokio::test]
    async fn turn_runner_fake() {
        let runner = FakeTurnRunner {
            completed: true,
            seen: std::sync::Mutex::new(Vec::new()),
        };
        let session = bridge_core::ids::SessionId::parse("implement-test").unwrap();

        assert!(
            runner
                .run_turn(
                    &session,
                    vec![bridge_core::domain::Part {
                        text: "hello".into()
                    }]
                )
                .await
        );
        assert_eq!(
            runner.seen.lock().unwrap().as_slice(),
            &[("implement-test".to_string(), vec!["hello".to_string()])]
        );
    }

    fn cr_entry(id: &str, mount: &str) -> AgentEntry {
        let mut e = acp_entry(id);
        e.kind = bridge_core::domain::AgentKind::ContainerRw;
        e.sandbox = Some(bridge_core::domain::SandboxConfig {
            runtime: None,
            image: "img".into(),
            mount: mount.into(),
            access: bridge_core::domain::MountAccess::Rw,
            egress: bridge_core::domain::EgressPolicy::Open,
            volumes: vec![],
        });
        e
    }
    fn snap(entries: Vec<AgentEntry>) -> bridge_core::domain::RegistrySnapshot {
        use bridge_core::ids::AgentId;
        bridge_core::domain::RegistrySnapshot {
            default: AgentId::parse("d").unwrap(),
            entries,
            allowed_cmds: vec![],
        }
    }
    fn wf(id: &str, agents: &[&str]) -> bridge_workflow::graph::WorkflowGraph {
        use bridge_core::ids::{AgentId, NodeId, WorkflowId};
        use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
        let nodes = agents
            .iter()
            .enumerate()
            .map(|(i, a)| WorkflowNode {
                id: NodeId::parse(format!("n{i}")).unwrap(),
                agent: AgentId::parse(*a).unwrap(),
                prompt_template: "{{input}}".into(),
                inputs: vec![],
            })
            .collect();
        WorkflowGraph {
            id: WorkflowId::parse(id).unwrap(),
            nodes,
        }
    }

    #[test]
    fn resolve_impl_identity_happy_and_rejects() {
        let s = snap(vec![cr_entry("impl", "/root")]);
        let edit = wf("edit", &["impl"]);
        let fix = wf("fix", &["impl"]);
        // happy with + without fix
        assert_eq!(
            resolve_impl_identity(&edit, Some(&fix), &s)
                .unwrap()
                .id
                .as_str(),
            "impl"
        );
        assert!(resolve_impl_identity(&edit, None, &s).is_ok());
        // edit multi-node
        assert!(
            resolve_impl_identity(&wf("edit", &["impl", "impl"]), Some(&fix), &s)
                .unwrap_err()
                .contains("single-node")
        );
        // fix multi-node
        assert!(
            resolve_impl_identity(&edit, Some(&wf("fix", &["impl", "impl"])), &s)
                .unwrap_err()
                .contains("fix workflow must be single-node")
        );
        // fix agent != edit agent
        assert!(
            resolve_impl_identity(&edit, Some(&wf("fix", &["other"])), &s)
                .unwrap_err()
                .contains("must match the edit agent")
        );
        // impl absent from snapshot
        assert!(resolve_impl_identity(&edit, Some(&fix), &snap(vec![]))
            .unwrap_err()
            .contains("not found"));
        // impl present but not ContainerRw
        assert!(
            resolve_impl_identity(&edit, Some(&fix), &snap(vec![acp_entry("impl")]))
                .unwrap_err()
                .contains("container_rw")
        );
    }

    #[test]
    fn rw_sweep_owner_matches_container_owner() {
        // spec §5 silent-leak guard: the recovery sweep owner MUST equal the warm backend's spawn-time
        // owner (both derive from container_owner over the SAME (config_path, mount, agent_id) triple).
        let cfg = std::path::Path::new("/cfg/a2a.toml");
        let s = snap(vec![cr_entry("impl", "/root")]);
        let targets = rw_sweep_targets(&s, cfg);
        assert_eq!(targets.len(), 1);
        assert_eq!(
            targets[0].1,
            container_owner(cfg, "/root", "impl"),
            "recovery-sweep owner must equal the backend spawn-time owner"
        );
    }

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
            mcp: vec![],
            mcp_delivery: Default::default(),
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
            acp_program_argv(&raw, None, &[], "/cwd").unwrap(),
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
        let (program, argv) = acp_program_argv(&sb, None, &[], "/cwd").unwrap();
        assert_eq!(program, "docker");
        assert_eq!(argv.last().unwrap(), "claude-agent-acp");
        assert!(argv.contains(&"/work:/work:ro".to_string()));
        // sandbox + a container name → the `--name` is spliced in (the :ro reaper path).
        let (_p, named) = acp_program_argv(&sb, Some("a2a-ro-owner-nonce"), &[], "/cwd").unwrap();
        assert!(named
            .windows(2)
            .any(|w| w == ["--name", "a2a-ro-owner-nonce"]));
        // missing cmd → ConfigInvalid.
        let mut nocmd = acp_entry("c");
        nocmd.cmd = None;
        assert!(acp_program_argv(&nocmd, None, &[], "/cwd").is_err());
    }

    #[test]
    fn acp_program_argv_appends_codex_native_mcp_args() {
        use bridge_core::mcp::{McpDelivery, McpServerSpec};
        // A CodexNative-delivery agent gets `-c mcp_servers.*` args appended ({cwd}-substituted);
        // an Acp-delivery agent (claude) does NOT (it gets MCP via the session/new param).
        let mut codex = acp_entry("codex");
        codex.cmd = Some("codex-acp".into());
        codex.mcp_delivery = McpDelivery::CodexNative;
        codex.mcp = vec![McpServerSpec {
            name: "prism".into(),
            command: "/opt/prism".into(),
            args: vec!["--repo".into(), "{cwd}".into()],
            env: vec![],
        }];
        let (_p, argv) = acp_program_argv(&codex, None, &[], "/repo/z").unwrap();
        assert!(argv.iter().any(|a| a == "-c"), "argv has -c: {argv:?}");
        assert!(
            argv.iter()
                .any(|a| a == r#"mcp_servers.prism.command="/opt/prism""#),
            "command override present: {argv:?}"
        );
        assert!(
            argv.iter().any(|a| a.contains("/repo/z")) && !argv.iter().any(|a| a.contains("{cwd}")),
            "{{cwd}} substituted: {argv:?}"
        );
        // Acp delivery (claude) does NOT append -c args here.
        let mut claude = acp_entry("claude");
        claude.mcp_delivery = McpDelivery::Acp;
        claude.mcp = codex.mcp.clone();
        let (_p, argv) = acp_program_argv(&claude, None, &[], "/repo/z").unwrap();
        assert!(!argv.iter().any(|a| a == "-c"), "no -c for Acp: {argv:?}");
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
    fn podman_example_parses_validates_and_mirrors_docker() {
        // The shipped podman example must parse, snapshot, satisfy the S3 runtime allowlist, and
        // differ from the docker example ONLY on the runtime/allowlist axes (no structural drift).
        let podman_src = include_str!("../../../examples/a2a-bridge.containerized.podman.toml");
        let docker_src = include_str!("../../../examples/a2a-bridge.containerized.toml");

        let cfg = config::RegistryConfig::parse(podman_src).expect("podman example parses");
        let snap = cfg.into_snapshot().expect("podman example snapshots");
        // S3 precondition (what Registry::new validates at boot): every sandbox runtime is allowlisted.
        assert!(
            snap.allowed_cmds.iter().any(|c| c == "podman"),
            "allowed_cmds must list podman"
        );
        for e in &snap.entries {
            if let Some(sb) = &e.sandbox {
                assert!(
                    snap.allowed_cmds.iter().any(|c| c == sb.runtime()),
                    "S3: sandbox runtime {:?} must be allowlisted",
                    sb.runtime()
                );
                assert_eq!(
                    sb.runtime(),
                    "podman",
                    "every sandbox uses the podman runtime"
                );
            }
        }

        // Parity (structural, ordered): strip the ONLY permitted differences — full-line comments, the
        // `runtime = …` lines, and the `allowed_cmds = …` line — and the remainders must be byte-identical.
        // This catches drift a line-SET diff misses: e.g. flipping a reader to `access = "rw"` changes a
        // structural line (an exact-remainder mismatch) even though `access = "rw"` exists elsewhere.
        let structural = |s: &str| -> Vec<String> {
            s.lines()
                .map(str::trim_end)
                .filter(|l| {
                    let t = l.trim();
                    !t.starts_with('#')
                        && !t.starts_with("runtime =")
                        && !t.starts_with("allowed_cmds =")
                })
                .map(String::from)
                .collect()
        };
        assert_eq!(
            structural(podman_src),
            structural(docker_src),
            "podman example must equal the docker example modulo comments + runtime/allowed_cmds lines"
        );
    }

    #[test]
    fn preflight_reports_unresponsive_runtimes_and_skips_empty() {
        use std::collections::BTreeSet;
        // Host-only config (no sandboxed runtimes) → nothing to report.
        assert!(missing_runtimes(&BTreeSet::new(), &|_| true).is_empty());
        let mut s = BTreeSet::new();
        s.insert("podman".to_string());
        // Probe fails → reported (the caller warns).
        assert_eq!(missing_runtimes(&s, &|_| false), vec!["podman".to_string()]);
        // Probe succeeds → not reported.
        assert!(missing_runtimes(&s, &|_| true).is_empty());
    }

    #[test]
    fn preflight_only_probes_allowlisted_runtimes() {
        // SECURITY (review BLOCKER): a sandbox runtime NOT in allowed_cmds must never reach the probe —
        // probing would execute a config-named binary outside the allowlist. into_snapshot doesn't enforce
        // S3 (Registry::new does), so such a config can reach preflight; runtimes_to_probe must drop it.
        let toml = r#"
default = "a"
allowed_cwd_root = "/tmp"
[server]
addr = "127.0.0.1:8080"
[registry]
allowed_cmds = ["podman"]
[[agents]]
id = "a"
cmd = "codex-acp"
[agents.sandbox]
runtime = "/tmp/evil-binary"
image = "img"
mount = "/tmp"
access = "ro"
egress = "open"
"#;
        let snap = config::RegistryConfig::parse(toml)
            .unwrap()
            .into_snapshot()
            .unwrap();
        let probe = runtimes_to_probe(&snap, None);
        assert!(
            !probe.iter().any(|r| r.contains("evil")),
            "un-allowlisted runtime must NOT be probed: {probe:?}"
        );
        assert!(probe.is_empty(), "only allowlisted runtimes are probed");
    }

    #[test]
    fn verify_runtime_gate_rejects_via_snapshot_allowlist() {
        // [registry]-less all-podman config: into_snapshot's default union is {podman} (the sandbox
        // runtime), so a defaulted-docker [verify] is rejected by the gate → VerifyOutcome::ConfigError
        // (no container spawns). Proves the snapshot -> gate -> run_verify_step wiring, runtime-free.
        let toml = r#"
default = "a"
allowed_cwd_root = "/tmp"
[server]
addr = "127.0.0.1:8080"
[[agents]]
id = "a"
cmd = "codex-acp"
[agents.sandbox]
runtime = "podman"
image = "img"
mount = "/tmp"
access = "ro"
egress = "open"
[verify]
image = "img"
cache = "c"
egress = "open"
[[verify.commands]]
name = "t"
cmd = "true"
"#;
        let cfg = config::RegistryConfig::parse(toml).expect("parses");
        let verify_cfg = cfg.verify.as_ref().map(|t| t.to_config());
        let snapshot = cfg.into_snapshot().expect("snapshots");
        assert!(
            snapshot.allowed_cmds.iter().any(|c| c == "podman")
                && !snapshot.allowed_cmds.iter().any(|c| c == "docker"),
            "default union is podman-only"
        );
        let gated = config::gate_verify_runtime(verify_cfg, &snapshot.allowed_cmds);
        let outcome = run_verify_step(
            &gated,
            &bridge_core::SessionCwd::parse("/tmp").unwrap(),
            std::path::Path::new("/tmp"),
        );
        assert!(
            matches!(outcome, verify::VerifyOutcome::ConfigError),
            "defaulted-docker verify under an all-podman allowlist → ConfigError"
        );
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

    #[test]
    fn reference_containerized_config_parses_and_loads() {
        // The config `implement` uses — parses, snapshot-validates (kind-aware), and its workflows load.
        // Also pins the codex impl default (effort=high) so a bad edit to that agent fails loud here.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/a2a-bridge.containerized.toml");
        let raw = std::fs::read_to_string(&path).unwrap();
        let cfg = config::RegistryConfig::parse(&raw).unwrap();
        let base = path.parent().unwrap();
        let wf = cfg.load_workflows(base).unwrap();
        // The named workflows agents reach for are present (count is incidental).
        for id in ["design", "code-review", "implement-edit", "implement-fix"] {
            assert!(
                wf.keys().any(|k| k.as_str() == id),
                "workflow {id:?} missing"
            );
        }
        let snap = cfg.into_snapshot().unwrap();
        let impl_agent = snap
            .entries
            .iter()
            .find(|e| e.id.as_str() == "impl")
            .expect("impl agent present");
        assert_eq!(impl_agent.cmd.as_deref(), Some("codex-acp"));
        assert_eq!(impl_agent.effort, Some(bridge_core::domain::Effort::High));
        assert_eq!(impl_agent.kind, bridge_core::domain::AgentKind::ContainerRw);
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
        match p.mode {
            ImplementMode::Fresh {
                task,
                repo,
                base_ref,
                workflow,
            } => {
                assert_eq!(task, "Add a FOO file");
                assert_eq!(repo, std::path::PathBuf::from("/src/repo"));
                assert_eq!(base_ref.as_deref(), Some("main"));
                assert_eq!(workflow, "implement-edit");
            }
            ImplementMode::Resume { .. } => panic!("expected Fresh"),
        }
        assert_eq!(p.config, std::path::PathBuf::from("c.toml"));
    }

    #[test]
    fn parse_implement_args_requires_task_and_repo() {
        // first token is a flag -> treated as missing <task>
        assert!(super::parse_implement_args(&["--repo".into(), "/r".into()]).is_err());
        // task present but no --repo
        assert!(super::parse_implement_args(&["task".into()]).is_err());
    }

    #[test]
    fn parse_implement_fresh_and_resume() {
        let fresh = super::parse_implement_args(&[
            "do X".into(),
            "--repo".into(),
            "/r".into(),
            "--config".into(),
            "/c.toml".into(),
        ])
        .unwrap();
        match fresh.mode {
            ImplementMode::Fresh { task, repo, .. } => {
                assert_eq!(task, "do X");
                assert_eq!(repo, std::path::PathBuf::from("/r"));
            }
            ImplementMode::Resume { .. } => panic!("expected Fresh"),
        }
        assert_eq!(fresh.config, std::path::PathBuf::from("/c.toml"));

        let res = super::parse_implement_args(&["--resume".into(), "impl-1-ab".into()]).unwrap();
        match res.mode {
            ImplementMode::Resume { resume_id } => assert_eq!(resume_id, "impl-1-ab"),
            ImplementMode::Fresh { .. } => panic!("expected Resume"),
        }
    }

    #[test]
    fn parse_implement_resume_rejects_repo() {
        assert!(super::parse_implement_args(&[
            "--resume".into(),
            "x".into(),
            "--repo".into(),
            "/r".into()
        ])
        .is_err());
        assert!(super::parse_implement_args(&["do X".into()]).is_err());
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
        assert!(
            wf.contains_key(&bridge_core::ids::WorkflowId::parse("implement-fix").unwrap()),
            "implement-fix workflow loads"
        );
        let snap = cfg.into_snapshot().unwrap();
        // Registry::new validates the snapshot WITHOUT spawning (lazy), so the real make_spawn_fn is never
        // called here — reuse it to avoid hand-rolling a typed no-op SpawnFn.
        let policy: std::sync::Arc<dyn bridge_core::ports::PolicyEngine> =
            std::sync::Arc::new(AutoPolicy);
        let run = bridge_core::run_identity::RunHandle {
            instance_id: "t".into(),
            host: "h".into(),
            lease: "/l/t.lock".into(),
            start: "0".into(),
        };
        let spawn = super::make_spawn_fn(
            policy,
            root.join("examples/a2a-bridge.containerized.toml"),
            run,
        );
        bridge_registry::registry::Registry::new(snap, spawn)
            .expect("containerized config (incl. the impl container_rw agent) validates");
    }
}
