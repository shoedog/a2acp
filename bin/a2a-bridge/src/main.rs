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
//   a2a-bridge run-batch <workflow> --manifest <file>    — submit a workflow batch to serve
//   a2a-bridge batch status|list|cancel                  — inspect/cancel serve-side batches
//   a2a-bridge submit <skill> --input <file> [--url <url>]
//                                                        — submit a detached task
//   a2a-bridge task get <id> [--url <url>]               — get task by id
//   a2a-bridge task list [--limit <n>] [--url <url>]     — list tasks
//   a2a-bridge task cancel <id> [--url <url>]            — cancel task by id
//   a2a-bridge task watch <id> [--from <seq>] [--url <url>]
//                                                        — stream a task's progress (SSE)
//   a2a-bridge task-spec schema|template|input           — inspect/validate typed task-spec inputs

mod catalog_probe;
mod config;
mod containers;
mod implement;
mod implement_resume;
mod merge;
mod resilient;
mod review;
mod route;
mod slice;
mod turn;
mod tweak;
mod verify;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bridge_a2a_inbound::server::InboundServer;
use bridge_a2a_outbound::{PeerDelegation, StubDelegation};
use bridge_core::domain::AgentEntry;
use bridge_core::error::BridgeError;
use bridge_core::permission::PermissionRegistry;
use bridge_core::ports::{AgentBackend, AgentRegistry, ConfigSource, DelegationPort, PolicyEngine};
use bridge_policy::{
    auth::AlwaysGrant,
    permission::{AutoPolicy, DeferPolicy},
};
use bridge_registry::registry::{Registry, SpawnFn};
use bridge_store::sqlite::SqliteStore;
use config::{FileConfigSource, RegistryConfig, ServerConfig};
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
  run-batch <workflow> Submit a manifest of independent workflow runs to a running serve.
                      --manifest <file> [--concurrency K] [--detach] [--url <url>]
  batch               Batch store.  status <id> | list | cancel <id>
  models              List each agent's advertised models/effort/modes (probed live).  [--config <f>] [--agent <id>] [--json]
  implement --input <file|-> Clone a repo, implement the task on a warm containerized agent, verify+review, hand off.
                      --repo <path> [--config <f>] [--base-ref <ref>] [--workflow <id>] [--merge [--onto <branch>]]
  merge <id>          Land an Approved run's commit into its source repo, re-authored to the operator
                      (Mode A: fast-forward --onto). [--config <f>] [--onto <branch>] [--force]
  init                Scaffold an a2a-bridge.toml + prompts.  --agents codex,claude [--dir <d>] [--force]
  serve               Run the A2A server.  [--config <path>]
  mcp                 Serve the MCP protocol over stdio (one stable Coordinator; A2A/CLI/MCP are thin adapters).
                      [--config <path>] [--store <path>]
  task-spec           Inspect or validate typed task-spec inputs. schema | template | input
  containers          List / reap this config's managed containers (crash-orphan cleanup).  list | reap
  submit              Send a unary message.  [skill] --input <file> [--context <id>] [--agent <id>] [--model <m>] [--effort <e>] [--mode <m>] [--cwd <dir>]
  task                Durable task store.  get | list | cancel | watch
  session             Warm session control.  status | release | cancel | clear | compact <contextId>

Run `a2a-bridge <subcommand> --help` for details. Quickstart + cwd/creds/concurrency notes: AGENTS.md.";

const MCP_USAGE: &str = "\
usage: a2a-bridge mcp [--config <path>] [--store <path>]

Serve the MCP protocol over stdio. STDOUT is reserved for NDJSON MCP replies; tracing is written to STDERR.

  --config <path>  registry config (default: ./a2a-bridge.toml)
  --store <path>   override the [store] path for this MCP process";

const TASK_SPEC_USAGE: &str = "\
usage: a2a-bridge task-spec schema [type]
       a2a-bridge task-spec template <type>
       a2a-bridge task-spec input <file|->

Inspect and validate typed task-spec inputs. Use `task-spec schema` to discover valid
task types and `task-spec template <type>` to scaffold one.";

#[derive(Debug, Clone, PartialEq, Eq)]
enum TopSubcommand {
    RunWorkflow,
    RunBatch,
    Batch,
    Models,
    Implement,
    Merge,
    Containers,
    Submit,
    Task,
    Session,
    Init,
    Mcp,
    TaskSpec,
    Help,
    Serve,
    Unknown(String),
}

fn parse_top_subcommand(raw_args: &[String]) -> TopSubcommand {
    match raw_args.get(1).map(|s| s.as_str()) {
        Some("run-workflow") => TopSubcommand::RunWorkflow,
        Some("run-batch") => TopSubcommand::RunBatch,
        Some("batch") => TopSubcommand::Batch,
        Some("models") => TopSubcommand::Models,
        Some("implement") => TopSubcommand::Implement,
        Some("merge") => TopSubcommand::Merge,
        Some("containers") => TopSubcommand::Containers,
        Some("submit") => TopSubcommand::Submit,
        Some("task") => TopSubcommand::Task,
        Some("session") => TopSubcommand::Session,
        Some("init") => TopSubcommand::Init,
        Some("mcp") => TopSubcommand::Mcp,
        Some("task-spec") => TopSubcommand::TaskSpec,
        Some("help") | Some("--help") | Some("-h") => TopSubcommand::Help,
        Some("serve") | None => TopSubcommand::Serve,
        Some(other) => TopSubcommand::Unknown(other.to_string()),
    }
}

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
        watchdog: entry.watchdog.clone(),
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
    permission_registry: Option<Arc<PermissionRegistry>>,
    perm_timeout_ms: u64,
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
        let mut be = bridge_acp::acp_backend::AcpBackend::spawn(program, &argv_ref, cfg)
            .await?
            .with_policy(Arc::clone(&self.policy));
        if let Some(reg) = &self.permission_registry {
            be = be
                .with_permission_registry(Arc::clone(reg))
                .with_permission_timeout_ms(self.perm_timeout_ms);
        }
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

fn worktree_owner(config_path: &std::path::Path, agent_id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let canonical =
        std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    canonical.to_string_lossy().hash(&mut h);
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
        watchdog: entry.watchdog.clone(),
        handshake_timeout: bridge_acp::acp_backend::AcpConfig::default().handshake_timeout,
        cancel_grace: bridge_acp::acp_backend::AcpConfig::default().cancel_grace,
        run: run.clone(),
        agent: entry.id.as_str().to_string(),
    })
}

#[derive(Clone)]
struct WorktreeRuntimeCfg {
    enabled: bool,
    root: String,
    allowed_root: Option<bridge_core::SessionCwd>,
}

fn default_worktrees_root() -> String {
    let root = std::env::var("HOME")
        .map(|home| PathBuf::from(home).join(".a2a-bridge").join("worktrees"))
        .unwrap_or_else(|_| PathBuf::from("/tmp").join("a2a-bridge").join("worktrees"));
    root.to_string_lossy().into_owned()
}

/// Resolve `[worktrees]` into a runtime cfg. Worktrees are opt-in, host-only, and require
/// `allowed_cwd_root` so the decorator can self-gate before any git operation. `[worktrees]`
/// changes require a serve restart because the spawn factory captures this config once;
/// hot-reload does not re-read it.
fn resolve_worktree_runtime_cfg(
    cfg: &RegistryConfig,
) -> Result<Option<WorktreeRuntimeCfg>, String> {
    let Some(w) = cfg.worktrees.as_ref().filter(|w| w.enabled) else {
        return Ok(None);
    };
    let allowed = cfg
        .allowed_cwd_root
        .as_deref()
        .ok_or_else(|| "[worktrees] enabled requires allowed_cwd_root".to_string())?;
    let allowed_root = bridge_core::SessionCwd::parse(allowed)
        .map_err(|e| format!("[worktrees]: invalid allowed_cwd_root: {e:?}"))?;
    let root = w.root.clone().unwrap_or_else(default_worktrees_root);
    config::preflight_worktrees_root(std::path::Path::new(&root), Some(&allowed_root))
        .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&root).map_err(|e| format!("[worktrees] root {root:?}: {e}"))?;
    Ok(Some(WorktreeRuntimeCfg {
        enabled: true,
        root,
        allowed_root: Some(allowed_root),
    }))
}

fn batch_runtime(
    cfg: &RegistryConfig,
) -> Result<Option<bridge_coordinator::BatchRuntime>, config::ConfigError> {
    Ok(cfg.batch_config()?.map(|batch| {
        bridge_coordinator::BatchRuntime::new(batch.max_concurrent, batch.default_concurrency)
    }))
}

/// The production `SpawnFn` (Acp compose-or-raw / Api / ContainerRw arms) — shared by run-workflow and the
/// `implement` subcommand so their registry builds can't drift. `owner_config_path` seeds the ContainerRw
/// owner token.
fn make_spawn_fn(
    policy_for_spawn: Arc<dyn bridge_core::ports::PolicyEngine>,
    owner_config_path: PathBuf,
    run: bridge_core::run_identity::RunHandle,
    permission_registry: Option<Arc<PermissionRegistry>>,
    perm_timeout_ms: u64,
    worktree_cfg: Option<WorktreeRuntimeCfg>,
) -> bridge_registry::registry::SpawnFn {
    Arc::new(move |entry: Arc<AgentEntry>| {
        let policy = Arc::clone(&policy_for_spawn);
        let owner_config_path = owner_config_path.clone();
        let run = run.clone();
        let permission_registry = permission_registry.clone();
        let worktree_cfg = worktree_cfg.clone();
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
                    let mut be =
                        bridge_acp::acp_backend::AcpBackend::spawn(&program, &argv_ref, acp)
                            .await?
                            .with_policy(policy);
                    if let Some(reg) = permission_registry.clone() {
                        be = be
                            .with_permission_registry(reg)
                            .with_permission_timeout_ms(perm_timeout_ms);
                    }
                    let inner = Arc::new(be) as Arc<dyn bridge_core::ports::AgentBackend>;
                    match &worktree_cfg {
                        Some(wc) if wc.enabled => {
                            let prov = Arc::new(bridge_worktree::host_git::HostGitWorktree::new());
                            let wcfg = bridge_worktree::provider_path::WorktreeConfig {
                                root: wc.root.clone(),
                                owner: worktree_owner(&owner_config_path, entry.id.as_str()),
                                run: run.instance_id.clone(),
                            };
                            let identity = bridge_worktree::backend::WorktreeIdentity {
                                run_id: run.instance_id.clone(),
                                host: run.host.clone(),
                                lease: run.lease.clone(),
                            };
                            Ok(Arc::new(bridge_worktree::backend::WorktreeBackend::new(
                                inner,
                                prov,
                                wcfg,
                                wc.allowed_root.clone(),
                                identity,
                            ))
                                as Arc<dyn bridge_core::ports::AgentBackend>)
                        }
                        _ => Ok(inner),
                    }
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
                            permission_registry: permission_registry.clone(),
                            perm_timeout_ms,
                        });
                    let be = bridge_container::ContainerRwBackend::new(ccfg, cspawn, owner).await?;
                    Ok(Arc::new(be) as Arc<dyn bridge_core::ports::AgentBackend>)
                }
            }
        })
    })
}

fn make_policy(server: &ServerConfig) -> Arc<dyn PolicyEngine> {
    match server.permission_policy.as_deref() {
        Some("defer") => Arc::new(DeferPolicy),
        _ => Arc::new(AutoPolicy),
    }
}

fn permission_timeout_ms(server: &ServerConfig) -> u64 {
    server.permission_timeout_ms.unwrap_or(120_000)
}

/// Parse `a2a-bridge run-workflow <id> --input <file|-> [--out <file>] [--config <path>]`
/// from a raw args iterator (skipping the binary name at position 0 and the
/// subcommand name at position 1).
const RUN_WORKFLOW_USAGE: &str = "\
usage: a2a-bridge run-workflow <workflow-id> --input <file|-> [--session-cwd <repo>] [--config <path>] [--out <file>]
       a2a-bridge run-workflow --serve [--url <url>] --context <context-id> <workflow-id> --input <file|-> [--session-cwd <repo>] [--out <file>]
  <workflow-id>   design | code-review | spec-review | plan-review | … (whatever your --config defines)
  --input <file|-> the typed task-spec markdown the workflow acts on (required; '-' reads stdin)
  --session-cwd   the repo the agents read/work in (per-request cwd; without it they use the launch cwd)
  --config <path> registry config (default: ./a2a-bridge.toml)
  --serve         call a running a2a-bridge serve via SendStreamingMessage instead of local execution
  --url <url>     serve URL for --serve (default: http://127.0.0.1:8080)
  --context       warm workflow parent context id for --serve (required with --serve)
  --out <file>    write the terminal node's output here (default: stdout)";

#[allow(clippy::type_complexity)]
fn parse_run_workflow_args(
    args: &[String],
) -> Result<
    (
        String,
        PathBuf,
        Option<PathBuf>,
        PathBuf,
        Option<String>,
        bool,
        String,
        Option<String>,
    ),
    BoxError,
> {
    let mut positionals: Vec<String> = Vec::new();
    let mut input: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut config: Option<PathBuf> = None;
    // The per-request ACP session cwd (the writable target for a container_rw agent, or the repo a
    // reader works in). Without it, run-workflow agents run in the LAUNCH cwd, not the target repo.
    let mut session_cwd: Option<String> = None;
    let mut serve = false;
    let mut url = "http://127.0.0.1:8080".to_string();
    let mut url_explicit = false;
    let mut context: Option<String> = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--serve" => {
                serve = true;
                idx += 1;
            }
            "--input" => {
                idx += 1;
                input = Some(PathBuf::from(
                    args.get(idx)
                        .ok_or("run-workflow: --input requires a value")?,
                ));
                idx += 1;
            }
            "--out" => {
                idx += 1;
                out = Some(PathBuf::from(
                    args.get(idx)
                        .ok_or("run-workflow: --out requires a value")?,
                ));
                idx += 1;
            }
            "--config" => {
                idx += 1;
                config = Some(PathBuf::from(
                    args.get(idx)
                        .ok_or("run-workflow: --config requires a value")?,
                ));
                idx += 1;
            }
            "--session-cwd" => {
                idx += 1;
                session_cwd = Some(
                    args.get(idx)
                        .ok_or("run-workflow: --session-cwd requires a value")?
                        .clone(),
                );
                idx += 1;
            }
            "--url" => {
                idx += 1;
                url_explicit = true;
                url = args
                    .get(idx)
                    .ok_or("run-workflow: --url requires a value")?
                    .clone();
                idx += 1;
            }
            "--context" => {
                idx += 1;
                context = Some(
                    args.get(idx)
                        .ok_or("run-workflow: --context requires a value")?
                        .clone(),
                );
                idx += 1;
            }
            other if other.starts_with("--") => {
                return Err(
                    format!("run-workflow: unknown flag {other:?}\n{RUN_WORKFLOW_USAGE}").into(),
                );
            }
            other => {
                positionals.push(other.to_string());
                idx += 1;
            }
        }
    }
    if positionals.len() != 1 {
        return Err(format!(
            "run-workflow: expected exactly one <workflow-id>, got {}\n{RUN_WORKFLOW_USAGE}",
            positionals.len()
        )
        .into());
    }
    let workflow_id = positionals.remove(0);
    if context.is_some() && !serve {
        return Err("run-workflow: --context requires --serve".into());
    }
    if url_explicit && !serve {
        return Err("run-workflow: --url requires --serve".into());
    }
    if serve && context.is_none() {
        return Err("run-workflow: --serve requires --context".into());
    }
    if serve && config.is_some() {
        return Err("run-workflow: --config cannot be used with --serve".into());
    }
    let input = input.ok_or_else(|| {
        format!("run-workflow: --input <file|-> is required\n{RUN_WORKFLOW_USAGE}")
    })?;
    let config = config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH));
    Ok((
        workflow_id,
        input,
        out,
        config,
        session_cwd,
        serve,
        url,
        context,
    ))
}

// ---------------------------------------------------------------------------
// `implement` subcommand (Slice B2b-1)
// ---------------------------------------------------------------------------

enum ImplementMode {
    Fresh {
        input: String,
        repo: PathBuf,
        base_ref: Option<String>,
        workflow: String,
    },
    Resume {
        resume_id: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum LangArg {
    Auto,
    Explicit(String),
    None,
}

struct ImplementArgs {
    mode: ImplementMode,
    config: PathBuf,
    /// `--merge`: after the run, land an Approved result into source_repo (ADR-0027). Approved-only sugar.
    merge: bool,
    /// `--onto <branch>`: the merge target (when `--merge`); else `[merge].target_ref` / `base_ref`.
    onto: Option<String>,
    /// `--depth auto|light|standard|thorough`: None means use `[review].default_depth`.
    depth: Option<review::Depth>,
    /// `--lang auto|none|<id>`: language profile selection for warm+verify.
    lang: LangArg,
}

const IMPLEMENT_USAGE: &str = "\
usage: a2a-bridge implement --input <file|-> --repo <path> [--config <path>] [--base-ref <ref>] [--workflow <id>] [--depth auto|light|standard|thorough]
       a2a-bridge implement --resume <id> [--config <path>]
  --input <file|-> task-spec markdown to implement; use '-' to read stdin (required)
  --repo <path>   the repo to implement in; cloned into a quarantine under allowed_cwd_root (required)
  --config <path> registry config defining the impl agent + [implement]/[verify]/[review] (default: ./a2a-bridge.toml)
  --base-ref      branch/SHA to start from (default: the repo HEAD)
  --workflow <id> the edit workflow (default: implement-edit)
  --depth         review depth: auto|light|standard|thorough (default: [review].default_depth, else auto)
  --lang          language profile: auto|none|<id> (default: auto; auto detects from repo markers)
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
        let mut depth = None;
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
                "--depth" => {
                    let val = args.get(i + 1).ok_or("implement: --depth needs a value")?;
                    depth = Some(review::Depth::parse_flag(val.as_str())?);
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
            depth,
            lang: LangArg::Auto,
        });
    }

    let (mut input, mut repo, mut base_ref, mut config, mut workflow) =
        (None, None, None, None, None);
    let mut merge = false;
    let mut onto = None;
    let mut depth: Option<review::Depth> = None;
    let mut lang = LangArg::Auto;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
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
            "--input" => {
                input = Some(
                    args.get(i + 1)
                        .ok_or("implement: --input needs a value")?
                        .clone(),
                );
                i += 2;
            }
            "--repo" => {
                repo = Some(PathBuf::from(
                    args.get(i + 1).ok_or("implement: --repo needs a value")?,
                ));
                i += 2;
            }
            "--base-ref" => {
                base_ref = Some(
                    args.get(i + 1)
                        .ok_or("implement: --base-ref needs a value")?
                        .clone(),
                );
                i += 2;
            }
            "--config" => {
                config = Some(PathBuf::from(
                    args.get(i + 1).ok_or("implement: --config needs a value")?,
                ));
                i += 2;
            }
            "--workflow" => {
                workflow = Some(
                    args.get(i + 1)
                        .ok_or("implement: --workflow needs a value")?
                        .clone(),
                );
                i += 2;
            }
            "--depth" => {
                let val = args.get(i + 1).ok_or("implement: --depth needs a value")?;
                depth = Some(review::Depth::parse_flag(val.as_str())?);
                i += 2;
            }
            "--lang" => {
                let val = args.get(i + 1).ok_or("implement: --lang needs a value")?;
                lang = match val.as_str() {
                    "auto" => LangArg::Auto,
                    "none" => LangArg::None,
                    s => LangArg::Explicit(s.to_string()),
                };
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("implement: unknown flag {other:?}\n{IMPLEMENT_USAGE}").into());
            }
            other => {
                return Err(format!(
                    "implement: unexpected positional arg {other:?}\n{IMPLEMENT_USAGE}"
                )
                .into());
            }
        }
    }
    Ok(ImplementArgs {
        mode: ImplementMode::Fresh {
            input: input.ok_or_else(|| {
                format!("implement: --input <file|-> is required\n{IMPLEMENT_USAGE}")
            })?,
            repo: repo.ok_or_else(|| {
                format!("implement: --repo <path> is required\n{IMPLEMENT_USAGE}")
            })?,
            base_ref,
            workflow: workflow.unwrap_or_else(|| "implement-edit".into()),
        },
        config: config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH)),
        merge,
        onto,
        depth,
        lang,
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

fn select_profile(
    cfg: &config::RegistryConfig,
    lang: &LangArg,
    repo: &std::path::Path,
) -> Result<Option<bridge_core::profile::LanguageProfile>, config::ConfigError> {
    let profiles = cfg.language_profiles()?;
    let by_id = |id: &str| profiles.iter().find(|p| p.id == id).cloned();
    match lang {
        LangArg::None => Ok(None),
        LangArg::Explicit(id) => by_id(id).map(Some).ok_or_else(|| {
            let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
            config::ConfigError::Registry(format!(
                "--lang {id:?}: no [[languages]] profile with that id; configured: {ids:?}"
            ))
        }),
        LangArg::Auto => match lsp_mcp::lang::detect(repo) {
            lsp_mcp::lang::Detection::Detected(l) => by_id(l.as_str()).map(Some).ok_or_else(|| {
                config::ConfigError::Registry(format!(
                    "detected {} but no [[languages]] id={:?} profile; add one or pass --lang none",
                    l.as_str(),
                    l.as_str()
                ))
            }),
            lsp_mcp::lang::Detection::None => Err(config::ConfigError::Registry(format!(
                "could not detect a language at {repo:?}; pass --lang <id|none> (configured: {:?})",
                profiles.iter().map(|p| p.id.as_str()).collect::<Vec<_>>()
            ))),
            lsp_mcp::lang::Detection::Ambiguous => Err(config::ConfigError::Registry(format!(
                "ambiguous repo root at {repo:?} (multiple language markers); pass --lang <id|none> (configured: {:?})",
                profiles.iter().map(|p| p.id.as_str()).collect::<Vec<_>>()
            ))),
        },
    }
}

/// Convert the persisted `resolved_lang` from a checkpoint back to a `LangArg` for `select_profile`.
/// `None` (pre-4b checkpoint) -> re-detect for backward compat; `Some("none")` -> bare run; `Some(id)` -> explicit.
fn resume_lang_arg(resolved_lang: &Option<String>) -> LangArg {
    match resolved_lang.as_deref() {
        None => LangArg::Auto,
        Some("none") => LangArg::None,
        Some(id) => LangArg::Explicit(id.to_string()),
    }
}

/// Run the B2b-2 verify once (total). `verify_cfg` was captured pre-snapshot. The verdict run itself never
/// fails (a runner error becomes a failed result); a config error reduces to `ConfigError`.
/// Returns `Skipped` immediately when `profile` is `None` (`--lang none`).
fn run_verify_step(
    verify_cfg: &Option<Result<config::VerifyConfig, config::ConfigError>>,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    clone_cwd: &bridge_core::SessionCwd,
    repo: &std::path::Path,
) -> verify::VerifyOutcome {
    let profile = match profile {
        None => {
            return verify::VerifyOutcome::Skipped {
                reason: "--lang none".into(),
            }
        }
        Some(p) => p,
    };
    match verify_cfg {
        None => verify::VerifyOutcome::NotConfigured,
        Some(Err(e)) => {
            eprintln!("[implement] verify: config error: {e:?} — skipping verify");
            verify::VerifyOutcome::ConfigError
        }
        Some(Ok(vcfg)) => {
            let repo_canon = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
            let cache_vol = verify::cache_volume_name(&vcfg.cache, &repo_canon.to_string_lossy());
            let image = profile.image.as_deref().unwrap_or(&vcfg.image);
            eprintln!(
                "[implement] verify: running {} command(s) in {}",
                profile.verify_commands.len(),
                image
            );
            let outcome = verify::run_verify(
                vcfg,
                Some(profile),
                clone_cwd,
                &cache_vol,
                &verify::docker_runner,
                16 * 1024,
            );
            if let verify::VerifyOutcome::Ran(ref verdict) = outcome {
                for r in &verdict.results {
                    if !r.ok {
                        eprintln!("[implement] verify: {} failed:\n{}", r.name, r.output);
                    }
                }
            }
            outcome
        }
    }
}

/// Warm the impl-lsp dep cache through verify's registries-only egress. Best-effort: failures degrade
/// in-container nav but never block the implement flow. Returns the cache name on success for later mounts.
/// Returns `None` immediately when `profile` is `None` (`--lang none`).
fn warm_lsp_deps_step(
    verify_cfg: &Option<Result<config::VerifyConfig, config::ConfigError>>,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    repo: &std::path::Path,
    clone: &std::path::Path,
    read_only: bool,
) -> Option<String> {
    let profile = profile?;
    let warn = |reason: String| {
        eprintln!(
            "[implement] lsp warm-deps skipped/failed: {reason} — in-container nav will be workspace-only"
        );
    };
    let vcfg = match verify_cfg {
        None => {
            warn("no [verify] block".into());
            return None;
        }
        Some(Err(e)) => {
            warn(format!("verify config error: {e:?}"));
            return None;
        }
        Some(Ok(vcfg)) => vcfg,
    };
    let (network, proxy) = match &vcfg.egress {
        bridge_core::domain::EgressPolicy::Locked { network, proxy, .. } => {
            (network.clone(), proxy.clone())
        }
        bridge_core::domain::EgressPolicy::Open => {
            warn("verify egress is open; locked network+proxy required".into());
            return None;
        }
    };
    // Key the cache on the SOURCE repo (mirrors verify::run_verify_step) so it is REUSED across runs and
    // bounded to one volume per repo. Keying on the per-run quarantine `clone` (a fresh nonce each run)
    // would never reuse the "warmed" deps AND orphan a GB-scale named volume every run (review BLOCKER).
    let repo_canon = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let cache_vol =
        verify::cache_volume_name(&profile.warm_cache_base, &repo_canon.to_string_lossy());
    // The fetch still runs IN the clone (its Cargo.lock == the source repo's at base_ref).
    let clone_canon = std::fs::canonicalize(clone).unwrap_or_else(|_| clone.to_path_buf());
    let clone_canon = clone_canon.to_string_lossy();
    let egress = implement::WarmEgress { network, proxy };
    let binding = profile.cache_binding(bridge_core::profile::CacheCtx::Fetch, &cache_vol, "");
    // Honor the [verify] runtime+image so the warm fetch tracks the same runtime as the rest of the
    // pipeline (the shipped podman config sets runtime="podman"; hardcoding docker silently degraded it).
    let runtime = vcfg.runtime.as_deref().unwrap_or("docker");
    let image = profile.image.as_deref().unwrap_or(&vcfg.image);
    let (program, argv) = implement::compose_warm_fetch(
        runtime,
        image,
        &clone_canon,
        &binding,
        &profile.fetch_cmd,
        &egress,
        read_only,
    );
    eprintln!("[implement] lsp warm-deps: fetching deps into {cache_vol}");
    match verify::docker_runner(&program, &argv) {
        Ok((0, _)) => {
            eprintln!("[implement] lsp warm-deps: ok ({cache_vol})");
            Some(cache_vol)
        }
        Ok((exit, output)) => {
            warn(format!(
                "fetch exited {exit}: {}",
                verify::truncate_output(&output, 4096).trim()
            ));
            None
        }
        Err(e) => {
            warn(format!("runner error: {e}"));
            None
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

/// Resolve `<base><suffix>` from the loaded workflows; warn + fall back to the base (standard) workflow
/// when the variant is absent. Slice presence still follows the TIER, not the resolved workflow.
fn variant_or_fallback(
    wf_map: &std::collections::HashMap<
        bridge_core::ids::WorkflowId,
        std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
    >,
    base: &bridge_core::ids::WorkflowId,
    suffix: &str,
) -> bridge_core::ids::WorkflowId {
    match bridge_core::ids::WorkflowId::parse(format!("{}{}", base.as_str(), suffix))
        .ok()
        .filter(|id| wf_map.contains_key(id))
    {
        Some(id) => id,
        None => {
            eprintln!(
                "[implement] review: no {}{} variant; falling back to standard workflow",
                base.as_str(),
                suffix
            );
            base.clone()
        }
    }
}

/// PURE. The production git args for review-depth sizing. Force rename detection so pure renames do not
/// degrade into full delete+add hunks when repo config disables `diff.renames`.
fn review_sizing_diff_args(base_sha: &str, head_sha: &str) -> Vec<String> {
    vec![
        "-c".into(),
        "core.quotePath=false".into(),
        "diff".into(),
        "--no-ext-diff".into(),
        "--no-textconv".into(),
        "--find-renames".into(),
        format!("{base_sha}..{head_sha}"),
    ]
}

/// PURE. Resume depth precedence: no `--depth` (None) uses the checkpoint's stored depth; an explicit
/// `--depth` overrides and becomes the new persisted depth (`Some(Auto)` clears a forced checkpoint).
fn resolve_resume_depth(flag: Option<review::Depth>, checkpoint: Option<&str>) -> review::Depth {
    match flag {
        None => review::Depth::from_forced_str(checkpoint),
        Some(d) => d,
    }
}

/// Run the B2b-3a review once (total). Returns `(outcome, synth_body)`. Fresh `CancellationToken` + `select!`
/// timeout→cancel→keep-drain PER call (so the `:ro` reaper still fires on a timed-out attempt). `run_id` is
/// qualified by `attempt`. `depth` selects the workflow tier (Auto=size-based; Forced overrides). `slice`
/// provides optional prism code-nav context for standard-tier reviews (degrades to None on failure).
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
    depth: review::Depth,
    slice: &dyn slice::SliceRunner,
) -> (review::ReviewOutcome, String) {
    let rcfg = match review_cfg {
        None => return (review::ReviewOutcome::NotConfigured, String::new()),
        Some(Err(e)) => {
            eprintln!("[implement] review: config error: {e:?}");
            return (review::ReviewOutcome::ConfigError, String::new());
        }
        Some(Ok(c)) => c,
    };

    // Size the committed diff (auto recompute each attempt). git/parse/timeout failure → standard (safe).
    let clone_path = std::path::Path::new(clone_cwd.as_str());
    let sizing: Option<(usize, usize)> = match tokio::time::timeout(
        rcfg.slice_timeout,
        tokio::process::Command::new("git")
            .current_dir(clone_path)
            .args(review_sizing_diff_args(base_sha, head_sha))
            .output(),
    )
    .await
    {
        Ok(Ok(o)) if o.status.success() => Some(review::parse_diff_for_depth(
            &String::from_utf8_lossy(&o.stdout),
        )),
        _ => {
            eprintln!("[implement] review: diff sizing failed or timed out; sizing unknown → standard tier");
            None
        }
    };
    let tier = depth.resolve(
        sizing,
        rcfg.light_max_lines,
        rcfg.light_max_files,
        rcfg.thorough_min_lines,
        rcfg.thorough_min_files,
    );

    // Slice prep for non-light tiers; write under .git/ (survives reset/clean). Degrade to None.
    let runid = format!("{task_id}-{attempt}");
    let slice_ref: Option<String> = if tier != review::Tier::Light {
        let refp = review::slice_ref_path(clone_path, &runid);
        match slice
            .produce(
                clone_path,
                base_sha,
                head_sha,
                &rcfg.slice_cmd,
                rcfg.slice_timeout,
            )
            .await
        {
            Some(body) => match slice::write_slice(&refp, &body, rcfg.slice_max_bytes) {
                Ok(()) => Some(format!(".git/a2a-bridge/review-slices/slice-{runid}.md")),
                Err(e) => {
                    eprintln!("[implement] review: slice write failed: {e}; continuing sliceless");
                    None
                }
            },
            None => {
                eprintln!("[implement] review: prism slice unavailable; continuing sliceless");
                None
            }
        }
    } else {
        None
    };

    // Select the workflow variant by tier: standard → base; light/thorough → <base>-<suffix> (fallback+warn).
    let graph_id = match tier {
        review::Tier::Standard => rcfg.workflow.clone(),
        review::Tier::Light | review::Tier::Thorough => {
            variant_or_fallback(wf_map, &rcfg.workflow, review::tier_workflow_suffix(tier))
        }
    };
    let Some(graph) = wf_map.get(&graph_id).cloned() else {
        return (review::ReviewOutcome::NotLoaded, String::new());
    };
    let input = review::build_review_input(task, base_sha, head_sha, slice_ref.as_deref());
    let ctx = bridge_workflow::executor::WorkflowRunContext {
        session_cwd: Some(clone_cwd.clone()),
        make_rich_sink: None,
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
    profile: Option<&'a bridge_core::profile::LanguageProfile>,
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
    /// Adaptive review depth: Auto = size-based; Forced overrides the auto-selection.
    depth: review::Depth,
}

#[async_trait::async_trait]
impl tweak::TweakEffects for ProdEffects<'_> {
    async fn verify(&mut self, _attempt: u32) -> verify::VerifyOutcome {
        run_verify_step(self.verify_cfg, self.profile, self.clone_cwd, self.repo)
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
            self.depth,
            &slice::ProdSliceRunner,
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

/// Merge `env` (the selected profile's Lsp env) onto the impl agent's `lsp` MCP server spec.
/// Profile values WIN over any same-key config value.
/// Other specs (e.g. prism) are untouched. No-op if there is no `lsp` spec.
fn apply_lsp_env(specs: &mut [bridge_core::mcp::McpServerSpec], env: &[(String, String)]) {
    if let Some(lsp) = specs.iter_mut().find(|s| s.name == "lsp") {
        // Profile env FIRST, then the config's non-overridden entries. Putting the selected-language
        // env ahead of the residual config env reproduces the pre-move render order byte-for-byte (the
        // old static config listed CARGO_* before LSP_MCP_LOG). Profile values WIN on key conflicts.
        let profile_keys: std::collections::HashSet<&str> =
            env.iter().map(|(k, _)| k.as_str()).collect();
        let mut merged: Vec<(String, String)> = env.to_vec();
        merged.extend(
            lsp.env
                .iter()
                .filter(|(k, _)| !profile_keys.contains(k.as_str()))
                .cloned(),
        );
        lsp.env = merged;
    }
}

/// Remove the `lsp` MCP server spec entirely (for `--lang none`: bare run, no in-container nav).
/// Other specs are untouched.
fn drop_lsp(specs: &mut Vec<bridge_core::mcp::McpServerSpec>) {
    specs.retain(|s| s.name != "lsp");
}

/// Inject the selected language profile's in-container LSP nav into a `container_rw` agent's `mcp` + sandbox
/// `volumes`: the profile's Lsp env (e.g. `CARGO_HOME=/cargo`, `CARGO_NET_OFFLINE=true`), the warmed dep
/// cache mount at `/cargo` (RO; only when `warm_cache_vol` is `Some` — i.e. the warm fetch succeeded), and a
/// writable per-repo target cache at `/lsp-target` (keyed on the SOURCE repo so it is reused, not leaked per
/// run). `profile == None` (`--lang none`) drops the lsp server. Shared by `build_warm_impl` (implement) and
/// the `run-workflow` pre-mutation so the two per-turn flows can't drift.
fn apply_warm_lsp(
    mcp: &mut Vec<bridge_core::mcp::McpServerSpec>,
    volumes: &mut Vec<String>,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    warm_cache_vol: Option<&str>,
    repo: &std::path::Path,
) {
    let target_canon = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let target_vol =
        verify::cache_volume_name("a2a-impl-lsp-target", &target_canon.to_string_lossy());
    match profile {
        None => drop_lsp(mcp),
        Some(p) => {
            let lsp = p.cache_binding(
                bridge_core::profile::CacheCtx::Lsp,
                warm_cache_vol.unwrap_or(""),
                "",
            );
            apply_lsp_env(mcp, &lsp.env);
            if warm_cache_vol.is_some() {
                volumes.extend(lsp.mounts);
            }
        }
    }
    volumes.push(format!("{target_vol}:/lsp-target"));
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
    impl_lsp_cache_vol: Option<&str>,
    profile: Option<&bridge_core::profile::LanguageProfile>,
    repo: &std::path::Path,
) -> Result<WarmImpl, BoxError> {
    let impl_entry =
        resolve_impl_identity(graph, fix_graph, snapshot).map_err(|e| format!("implement: {e}"))?;
    let edit_template = graph.nodes[0].prompt_template.clone();
    let fix_template = fix_graph.map(|g| g.nodes[0].prompt_template.clone());
    // Fail fast with a clear message if the claude impl agent's mounted OAuth creds are already expired,
    // instead of spawning a container that crashes opaquely on session/prompt (the recurring footgun).
    if let Some(sb) = impl_entry.sandbox.as_ref() {
        implement::claude_cred_preflight(
            impl_entry.cmd.as_deref(),
            &sb.volumes,
            implement::now_ms(),
        )?;
    }
    let mut ccfg = container_rw_cfg_from_entry(&impl_entry, run)?;
    // Slice B: runtime-derived LSP nav mounts for the in-container implementor's rust-analyzer (these
    // back the lsp MCP server delivered into this container via the ADR-0028 CodexNative path).
    // These are NAMED volumes mounting at absolute container paths OUTSIDE the `:rw` repo mount, so they
    // never nest under it (the S6 concern is host paths under `mount`; the parse/validate S6 ran on the
    // static config). Appended here (post-snapshot) because the names are per-clone (NOT static, codex #1):
    //   - the warmed dep cache at /cargo, READ-ONLY (Task 1 offline proof: read-only is sufficient) — only
    //     when the warm fetch succeeded (else in-container nav degrades to workspace-only, as warned).
    //   - a writable target dir at /lsp-target so RA's own CARGO_TARGET_DIR persists per-repo across runs.
    // Keyed on the SOURCE repo (like the dep cache + verify), NOT the per-run clone — so it is REUSED and
    // bounded to one volume per repo (review BLOCKER: clone-keying never reuses + leaks a volume per run).
    apply_warm_lsp(
        &mut ccfg.mcp,
        &mut ccfg.sandbox.volumes,
        profile,
        impl_lsp_cache_vol,
        repo,
    );
    let warm_owner = container_owner(
        owner_config_path,
        ccfg.sandbox.mount.as_str(),
        impl_entry.id.as_str(),
    );
    let cspawn = Arc::new(AcpContainerSpawn {
        policy,
        permission_registry: None,
        perm_timeout_ms: 120_000,
    }) as Arc<dyn bridge_container::ContainerSpawn>;
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
    profile: Option<&bridge_core::profile::LanguageProfile>,
    review_cfg: &Option<Result<config::ReviewConfig, config::ConfigError>>,
    wf_map: &std::collections::HashMap<
        bridge_core::ids::WorkflowId,
        std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
    >,
    executor: &bridge_workflow::executor::WorkflowExecutor,
    fix_template: Option<String>,
    prod_ckpt: &mut implement_resume::ProdCheckpoint,
    depth: review::Depth,
) -> implement_resume::ImplementPhase {
    let final_ = {
        let mut effects = ProdEffects {
            verify_cfg,
            profile,
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
            depth,
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

fn write_implement_task_file(
    clone: &std::path::Path,
    spec: &bridge_core::task_spec::TaskSpec,
) -> Result<(), BoxError> {
    std::fs::write(
        clone.join(".git").join("A2A_TASK.md"),
        bridge_core::task_spec::body(spec).as_bytes(),
    )
    .map_err(|e| format!("implement: write task file: {e}").into())
}

/// `a2a-bridge implement --input <file|-> --repo <path>` — clone a quarantine, run the 1-node `implement-edit`
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
    let depth = a.depth;
    let lang = a.lang;
    let (input, repo, base_ref, workflow) = match a.mode {
        ImplementMode::Fresh {
            input,
            repo,
            base_ref,
            workflow,
        } => (input, repo, base_ref, workflow),
        ImplementMode::Resume { resume_id } => {
            return implement_resume_cmd(
                &resume_id,
                &config_path,
                merge_requested,
                onto.as_deref(),
                depth,
            )
            .await;
        }
    };
    let raw_input = read_input(&input)?;
    let spec = bridge_core::task_spec::validate_input(&raw_input)
        .map_err(|e| format!("implement: {e}"))?;
    let task = bridge_core::task_spec::body(&spec).to_string();

    // 1. config + canonical allowed_cwd_root (the ContainerRw mount anchor).
    let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("implement: read config {:?}: {e}", config_path))?;
    let cfg =
        config::RegistryConfig::parse(&raw).map_err(|e| format!("implement: config parse: {e}"))?;
    let worktree_cfg = resolve_worktree_runtime_cfg(&cfg).map_err(|e| format!("implement: {e}"))?;
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
    let profile = select_profile(&cfg, &lang, &repo)
        .map_err(|e| format!("implement: language profile: {e}"))?;
    // B2b-3a: parsed pre-commit (beside verify).
    let review_cfg = cfg.review.as_ref().map(|t| t.to_config());
    // ADR-0027: parsed pre-move (--merge sugar).
    let merge_cfg = cfg.merge.as_ref().map(|m| m.to_config());
    // Owner default: an absent --depth falls through to [review].default_depth (else Auto).
    let default_depth = match &review_cfg {
        Some(Ok(rc)) => rc.default_depth,
        _ => review::Depth::Auto,
    };
    let depth = depth.unwrap_or(default_depth);
    let mut snapshot = cfg
        .into_snapshot()
        .map_err(|e| format!("implement: snapshot: {e}"))?;
    // Stamp the clone cwd into every entry's `session_cwd` so that codex's native MCP `{cwd}`
    // (baked at spawn from the static entry) points at the clone, not the launch cwd.  Mirrors
    // `run_workflow_cmd`'s ADR-0028 stamping exactly — stamping all entries is proven-safe there.
    for e in &mut snapshot.entries {
        e.session_cwd = Some(clone_cwd.as_str().to_string());
    }
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
    if let Some(wc) = &worktree_cfg {
        bridge_worktree::sweep::sweep_orphans(
            &wc.root,
            &host,
            &bridge_core::liveness::FsLeaseProbe,
        );
    }
    // Label-scoped END-sweep backstop (THIS run's `a2a.run` only). Declared BEFORE `warm` → drops AFTER it
    // (the warm `retire` reaps first; this catches anything it missed, including the `:ro` reviewers).
    let _run_guard = RunEndGuard {
        runtimes: run_guard_runtimes(&snapshot, &owner_config_path),
        instance_id: instance_id.clone(),
    };
    let _wt_run_guard = worktree_cfg.as_ref().and_then(|wc| {
        wc.enabled
            .then(|| bridge_worktree::sweep::WorktreeRunEndGuard {
                root: wc.root.clone(),
                instance_id: run.instance_id.clone(),
            })
    });
    let policy: Arc<dyn PolicyEngine> = Arc::new(AutoPolicy);
    let impl_lsp_cache_vol =
        warm_lsp_deps_step(&verify_cfg, profile.as_ref(), &repo, &clone, false);

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
        impl_lsp_cache_vol.as_deref(),
        profile.as_ref(),
        &repo,
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
    let spawn = make_spawn_fn(
        Arc::clone(&policy),
        owner_config_path,
        run,
        None,
        120_000,
        worktree_cfg.clone(),
    );
    let registry = Arc::new(
        bridge_registry::registry::Registry::new(snapshot, spawn)
            .map_err(|e| format!("implement: registry: {e:?}"))?,
    );
    let executor = bridge_workflow::executor::WorkflowExecutor::new(
        Arc::clone(&registry) as Arc<dyn bridge_core::ports::AgentRegistry>
    );

    // Deliver the task to the agent via a FILE in the clone, NOT the ACP session/prompt payload: a large
    // task that contains non-ASCII chars crashes the in-container claude session/prompt (the large x
    // non-ASCII interaction; ASCII or small is fine). Writing it to `.git/A2A_TASK.md` keeps the prompt
    // small + ASCII while the agent reads the full (arbitrarily large / unicode) task from the file
    // (file-read of unicode is safe — validated). implement-edit.md reads this file instead of {{input}}.
    write_implement_task_file(&clone, &spec)?;
    // First edit turn — on the WARM session (off the executor). The edit template now points the agent at
    // `.git/A2A_TASK.md` (no task interpolation), so the prompt itself is small + ASCII regardless of task.
    let edit_vars: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
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
    let msg = implement::commit_message(
        spec.section("Commit Message").map(|s| s.content.clone()),
        implement::read_commit_msg_file(&clone),
        spec.title.as_deref().unwrap_or(""),
        &task,
    );
    if msg.1 == implement::CommitSource::Derived {
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
                    forced_depth: depth.to_forced_str(),
                    resolved_lang: Some(
                        profile
                            .as_ref()
                            .map(|p| p.id.clone())
                            .unwrap_or_else(|| "none".to_string()),
                    ),
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
                profile.as_ref(),
                &review_cfg,
                &wf_map,
                &executor,
                fix_template,
                &mut prod_ckpt,
                depth,
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
    depth_override: Option<review::Depth>,
) -> Result<(), BoxError> {
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("implement --resume: read config {config_path:?}: {e}"))?;
    let cfg = config::RegistryConfig::parse(&raw)
        .map_err(|e| format!("implement --resume: config parse: {e}"))?;
    let worktree_cfg =
        resolve_worktree_runtime_cfg(&cfg).map_err(|e| format!("implement --resume: {e}"))?;
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
    let resume_lang = resume_lang_arg(&ck.resolved_lang);
    let profile = select_profile(&cfg, &resume_lang, &ck.source_repo)
        .map_err(|e| format!("implement: language profile (resume): {e}"))?;
    let review_cfg = cfg.review.as_ref().map(|t| t.to_config());
    let merge_cfg = cfg.merge.as_ref().map(|m| m.to_config()); // ADR-0027: parsed pre-move (--merge sugar)
    let mut snapshot = cfg
        .into_snapshot()
        .map_err(|e| format!("implement --resume: snapshot: {e}"))?;
    // Stamp the clone cwd into every entry's `session_cwd` (mirrors implement_cmd + run_workflow_cmd).
    // Ensures codex's native MCP `{cwd}` (baked from the static entry at spawn) targets the clone,
    // not the launch cwd — so prism indexes the right repo during the review step.
    for e in &mut snapshot.entries {
        e.session_cwd = Some(clone_cwd.as_str().to_string());
    }
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
    if let Some(wc) = &worktree_cfg {
        bridge_worktree::sweep::sweep_orphans(
            &wc.root,
            &host,
            &bridge_core::liveness::FsLeaseProbe,
        );
    }
    let _run_guard = RunEndGuard {
        runtimes: run_guard_runtimes(&snapshot, &owner_config_path),
        instance_id: instance_id.clone(),
    };
    let _wt_run_guard = worktree_cfg.as_ref().and_then(|wc| {
        wc.enabled
            .then(|| bridge_worktree::sweep::WorktreeRunEndGuard {
                root: wc.root.clone(),
                instance_id: run.instance_id.clone(),
            })
    });

    let policy: Arc<dyn PolicyEngine> = Arc::new(AutoPolicy);
    let impl_lsp_cache_vol = warm_lsp_deps_step(
        &verify_cfg,
        profile.as_ref(),
        &ck.source_repo,
        &clone,
        false,
    );
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
        impl_lsp_cache_vol.as_deref(),
        profile.as_ref(),
        &ck.source_repo,
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

    let spawn = make_spawn_fn(
        Arc::clone(&policy),
        owner_config_path,
        run,
        None,
        120_000,
        worktree_cfg.clone(),
    );
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
    // Resume precedence (replay-correct): None -> checkpoint; Some(d) -> override and persist
    // (Some(Auto) clears a forced checkpoint). forced_depth=None re-sizes each attempt.
    let depth = resolve_resume_depth(depth_override, ck.forced_depth.as_deref());
    if depth_override.is_some() {
        prod_ckpt.ck.forced_depth = depth.to_forced_str();
        let _ = implement_resume::save_checkpoint(&clone, &prod_ckpt.ck);
    }
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
        profile.as_ref(),
        &review_cfg,
        &wf_map,
        &executor,
        fix_template,
        &mut prod_ckpt,
        depth,
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
fn build_run_workflow_streaming_request(
    workflow_id: &str,
    input: &str,
    context: &str,
    session_cwd: Option<&str>,
) -> serde_json::Value {
    let mut metadata = serde_json::Map::new();
    metadata.insert("a2a-bridge.skill".to_string(), workflow_id.into());
    if let Some(cwd) = session_cwd {
        metadata.insert("a2a-bridge.cwd".to_string(), cwd.into());
    }

    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": a2a::methods::SEND_STREAMING_MESSAGE,
        "params": {
            "message": {
                "contextId": context,
                "taskId": a2a::new_task_id(),
                "metadata": metadata,
                "parts": [
                    {
                        "kind": "text",
                        "text": input
                    }
                ]
            }
        }
    })
}

fn collect_artifact_text(artifact: &a2a::Artifact) -> String {
    artifact
        .parts
        .iter()
        .filter_map(|part| part.as_text())
        .collect::<Vec<_>>()
        .join("")
}

fn message_text(message: &a2a::Message) -> String {
    message
        .parts
        .iter()
        .filter_map(|part| part.as_text())
        .collect::<Vec<_>>()
        .join("")
}

async fn run_workflow_serve_client(
    workflow_id: &str,
    input: &str,
    out_path: Option<&Path>,
    url: &str,
    context: &str,
    session_cwd: Option<&str>,
) -> Result<(), BoxError> {
    let body = build_run_workflow_streaming_request(workflow_id, input, context, session_cwd);
    let resp = reqwest::Client::new()
        .post(url)
        .header(a2a::SVC_PARAM_VERSION, a2a::VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            format!("cannot reach serve at {url} — is `a2a-bridge serve` running? ({e})")
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        eprintln!("error: HTTP {status}\n{text}");
        return Err(format!("run-workflow --serve: server returned {status}").into());
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if content_type
        .split(';')
        .any(|part| part.trim().eq_ignore_ascii_case("application/json"))
    {
        let text = resp.text().await.unwrap_or_default();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(err) = v.get("error") {
                return Err(format!("run-workflow --serve failed: {err}").into());
            }
        }
        return Err("run-workflow --serve: expected SSE response, got JSON response".into());
    }

    use futures::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut output = String::new();
    let mut terminal: Option<a2a::TaskState> = None;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("run-workflow --serve: stream error: {e}"))?;
        let text = String::from_utf8_lossy(&chunk);
        buf.push_str(&text);

        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim_end_matches('\r').to_string();
            buf.drain(..=pos);

            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = data.trim_start();
            let response: a2a::StreamResponse = serde_json::from_str(payload)
                .map_err(|e| format!("run-workflow --serve: bad SSE data frame: {e}"))?;
            match response {
                a2a::StreamResponse::ArtifactUpdate(update) => {
                    output.push_str(&collect_artifact_text(&update.artifact));
                }
                a2a::StreamResponse::StatusUpdate(update) => {
                    eprintln!("[workflow] status {:?}", update.status.state);
                    if let Some(message) = &update.status.message {
                        let text = message_text(message);
                        if !text.is_empty() {
                            eprintln!("{text}");
                        }
                    }
                    if update.status.message.is_none()
                        && matches!(
                            update.status.state,
                            a2a::TaskState::Completed
                                | a2a::TaskState::Failed
                                | a2a::TaskState::Canceled
                        )
                    {
                        terminal = Some(update.status.state);
                    }
                }
                a2a::StreamResponse::Message(message) => {
                    let text = message_text(&message);
                    if !text.is_empty() {
                        eprintln!("{text}");
                    }
                }
                a2a::StreamResponse::Task(task) => {
                    eprintln!("[workflow] task state {:?}", task.status.state);
                }
            }
        }
    }

    if let Some(out) = out_path {
        std::fs::write(out, &output)
            .map_err(|e| format!("run-workflow: cannot write output {:?}: {e}", out))?;
    } else {
        print!("{output}");
    }

    match terminal {
        Some(a2a::TaskState::Completed) => Ok(()),
        Some(a2a::TaskState::Failed) | Some(a2a::TaskState::Canceled) => {
            Err("run-workflow --serve: workflow did not complete successfully".into())
        }
        Some(other) => {
            Err(format!("run-workflow --serve: non-terminal final state {other:?}").into())
        }
        None => Err("run-workflow --serve: stream ended without terminal status".into()),
    }
}

async fn run_workflow_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{RUN_WORKFLOW_USAGE}");
        return Ok(());
    }
    bridge_observ::init();
    let (workflow_id, input_path, out_path, config_path, session_cwd, serve, url, context) =
        parse_run_workflow_args(args)?;

    let input = read_input(&input_path.to_string_lossy())
        .map_err(|e| format!("run-workflow: cannot read input {:?}: {e}", input_path))?;
    if let Err(e) = bridge_core::task_spec::validate_input(&input) {
        eprintln!("{}", e.client_message());
        return Err(e.into());
    }

    if serve {
        let context = context.expect("parse_run_workflow_args requires --context with --serve");
        return run_workflow_serve_client(
            &workflow_id,
            &input,
            out_path.as_deref(),
            &url,
            &context,
            session_cwd.as_deref(),
        )
        .await;
    }

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

    // #1d: warm the in-container LSP dep cache up front for container_rw agents. run-workflow targets ONE
    // repo (the stamped --session-cwd), so the profile + warm are resolved once and applied to every
    // container_rw entry. Best-effort: any failure degrades to no in-container nav (Part A reports it).
    let verify_cfg_raw = cfg.verify.as_ref().map(|t| t.to_config());
    let warm_repo: Option<std::path::PathBuf> = session_cwd.as_ref().map(std::path::PathBuf::from);
    let warm_profile = match warm_repo.as_ref() {
        Some(repo) => match select_profile(&cfg, &LangArg::Auto, repo) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[run-workflow] lsp warm: language detect skipped: {e:?}");
                None
            }
        },
        None => None,
    };

    let worktree_cfg =
        resolve_worktree_runtime_cfg(&cfg).map_err(|e| format!("run-workflow: {e}"))?;

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
    // Warm ONLY container_rw agents the SELECTED workflow actually resolves (its graph nodes), not every
    // container_rw entry in the config: the shipped containerized.toml defines the `impl` container_rw agent
    // AND host-only workflows (code-review/spec-review/design) — warming config-wide would make those pay a
    // docker warm-fetch + registries-egress attempt for an agent they never spawn (dual-review MAJOR).
    let wf_agent_ids: std::collections::HashSet<&str> =
        graph.nodes.iter().map(|n| n.agent.as_str()).collect();
    let warms_container_rw = snapshot.entries.iter().any(|e| {
        e.kind == bridge_core::domain::AgentKind::ContainerRw
            && wf_agent_ids.contains(e.id.as_str())
    });
    if warms_container_rw {
        if let (Some(repo), Some(p)) = (warm_repo.as_ref(), warm_profile.as_ref()) {
            let verify_cfg = config::gate_verify_runtime(verify_cfg_raw, &snapshot.allowed_cmds);
            // read_only=true: the per-turn "clone" is the user's REAL repo — never mutate it.
            let warm_vol = warm_lsp_deps_step(&verify_cfg, Some(p), repo, repo, true);
            for e in &mut snapshot.entries {
                if e.kind == bridge_core::domain::AgentKind::ContainerRw
                    && wf_agent_ids.contains(e.id.as_str())
                {
                    if let Some(sb) = e.sandbox.as_mut() {
                        apply_warm_lsp(
                            &mut e.mcp,
                            &mut sb.volumes,
                            Some(p),
                            warm_vol.as_deref(),
                            repo,
                        );
                    }
                }
            }
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
    if let Some(wc) = &worktree_cfg {
        bridge_worktree::sweep::sweep_orphans(
            &wc.root,
            &host,
            &bridge_core::liveness::FsLeaseProbe,
        );
    }
    let _wt_run_guard = worktree_cfg.as_ref().and_then(|wc| {
        wc.enabled
            .then(|| bridge_worktree::sweep::WorktreeRunEndGuard {
                root: wc.root.clone(),
                instance_id: run.instance_id.clone(),
            })
    });
    let policy = Arc::new(bridge_policy::permission::AutoPolicy);
    let policy_for_spawn = Arc::clone(&policy) as Arc<dyn bridge_core::ports::PolicyEngine>;
    let spawn = make_spawn_fn(
        policy_for_spawn,
        owner_config_path,
        run,
        None,
        120_000,
        worktree_cfg,
    );
    let registry = Arc::new(
        bridge_registry::registry::Registry::new(snapshot, spawn)
            .map_err(|e| format!("run-workflow: registry init error: {e:?}"))?,
    );
    let executor = bridge_workflow::executor::WorkflowExecutor::new(
        Arc::clone(&registry) as Arc<dyn bridge_core::ports::AgentRegistry>
    );

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
                make_rich_sink: None,
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
                node,
                ok: node_ok,
                usage: _,
                ..
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
// `models` subcommand — list each agent's advertised models/effort/modes (probed live)
// ---------------------------------------------------------------------------

const MODELS_USAGE: &str = "\
usage: a2a-bridge models [--config <path>] [--agent <id>] [--json]
  List each configured agent's advertised models (+ effort levels + modes), probed live host-side.
  Pass one of these to the per-request override (message.metadata a2a-bridge.{model,effort,mode}).
  --config <path>  registry config (default: ./a2a-bridge.toml)
  --agent <id>     show only this agent
  --json           emit JSON (same shape as the card's agent-models extension params.agents)";

struct ModelsArgs {
    config: Option<String>,
    agent: Option<String>,
    json: bool,
}

fn parse_models_args(args: &[String]) -> Result<ModelsArgs, BoxError> {
    let mut config: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut json = false;
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--config" => {
                config = Some(
                    iter.next()
                        .ok_or("models: --config requires a value")?
                        .clone(),
                );
            }
            "--agent" => {
                agent = Some(
                    iter.next()
                        .ok_or("models: --agent requires a value")?
                        .clone(),
                );
            }
            "--json" => json = true,
            other => {
                return Err(format!("models: unknown flag {other:?}\n{MODELS_USAGE}").into());
            }
        }
    }
    Ok(ModelsArgs {
        config,
        agent,
        json,
    })
}

/// The catalog as the card's `params.agents` JSON object (each value via the shared `caps_to_json`).
fn catalog_to_json(catalog: &bridge_core::catalog::ModelCatalog) -> serde_json::Value {
    let agents: serde_json::Map<String, serde_json::Value> = catalog
        .iter()
        .map(|(id, caps)| (id.clone(), bridge_core::catalog::caps_to_json(caps)))
        .collect();
    serde_json::Value::Object(agents)
}

async fn models_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{MODELS_USAGE}");
        return Ok(());
    }
    bridge_observ::init();
    let parsed = parse_models_args(args)?;
    let config_path = parsed
        .config
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(CONFIG_PATH));
    let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("models: cannot read config {:?}: {e}", config_path))?;
    let cfg = config::RegistryConfig::parse(&raw)
        .map_err(|e| format!("models: config parse error: {e}"))?;
    let snapshot = cfg
        .into_snapshot()
        .map_err(|e| format!("models: registry snapshot error: {e}"))?;
    // (id, entry) pairs, optionally filtered to one agent. Probing is on-demand (separate process from
    // serve, so always live) and degrades per-agent.
    let entries: Vec<(String, AgentEntry)> = snapshot
        .entries
        .iter()
        .map(|e| (e.id.as_str().to_string(), e.clone()))
        .filter(|(id, _)| parsed.agent.as_ref().is_none_or(|want| want == id))
        .collect();
    if entries.is_empty() {
        if let Some(want) = &parsed.agent {
            return Err(format!("models: no agent {want:?} in {config_path:?}").into());
        }
        return Err(format!("models: no agents configured in {config_path:?}").into());
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
    let catalog = catalog_probe::probe_all(&entries, &cwd).await;
    if parsed.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&catalog_to_json(&catalog))?
        );
    } else {
        for (id, _) in &entries {
            match catalog.get(id) {
                Some(caps) => {
                    let current = caps.current_model.as_deref().unwrap_or("?");
                    println!("{id}: {}  (current: {current})", caps.models.join(", "));
                    if !caps.effort_levels.is_empty() {
                        println!("    effort: {}", caps.effort_levels.join(", "));
                    }
                    if !caps.modes.is_empty() {
                        println!("    modes:  {}", caps.modes.join(", "));
                    }
                }
                None => println!("{id}: unavailable (probe failed — see logs)"),
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// A2A client helpers: submit + task get/list/cancel + batch
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

const RUN_BATCH_USAGE: &str = "\
usage: a2a-bridge run-batch <workflow> --manifest <file> [--concurrency K] [--detach] [--url <url>]

Submit a batch to a running a2a-bridge serve. The manifest is TOML:
  [[item]]
  id = \"optional-item-id\"
  input = \"inline prompt\"
  session_cwd = \"/optional/repo\"

Use input_file instead of input to inline file contents relative to the manifest directory.

Each item input is a typed task-spec (YAML front-matter `task-type` + a markdown body),
validated before the batch is created. See `a2a-bridge task-spec schema`; scaffold one with
`a2a-bridge task-spec template <type>` (`freeform` is the catch-all).";

const BATCH_USAGE: &str = "\
usage: a2a-bridge batch status <batch-id> [--url <url>]
       a2a-bridge batch list [--url <url>]
       a2a-bridge batch cancel <batch-id> [--url <url>]";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct BatchWireItem {
    item_id: String,
    input: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_cwd: Option<String>,
}

#[derive(serde::Deserialize)]
struct BatchManifest {
    #[serde(default)]
    item: Vec<BatchManifestItem>,
}

#[derive(serde::Deserialize)]
struct BatchManifestItem {
    id: Option<String>,
    input: Option<String>,
    input_file: Option<String>,
    session_cwd: Option<String>,
}

fn parse_batch_manifest(toml_str: &str, base_dir: &Path) -> Result<Vec<BatchWireItem>, BoxError> {
    let manifest: BatchManifest =
        toml::from_str(toml_str).map_err(|e| format!("run-batch: invalid manifest TOML: {e}"))?;
    if manifest.item.is_empty() {
        return Err("run-batch: manifest must contain at least one [[item]]".into());
    }

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(manifest.item.len());
    for (idx, item) in manifest.item.into_iter().enumerate() {
        let item_id = item.id.unwrap_or_else(|| idx.to_string());
        if item_id.is_empty() {
            return Err("run-batch: item id must not be empty".into());
        }
        if !seen.insert(item_id.clone()) {
            return Err(format!("run-batch: duplicate item id {item_id:?}").into());
        }
        let input = match (item.input, item.input_file) {
            (Some(_), Some(_)) => {
                return Err(format!(
                    "run-batch: item {item_id:?} must set exactly one of input/input_file"
                )
                .into());
            }
            (None, None) => {
                return Err(format!(
                    "run-batch: item {item_id:?} must set exactly one of input/input_file"
                )
                .into());
            }
            (Some(input), None) => input,
            (None, Some(input_file)) => {
                let path = base_dir.join(input_file);
                std::fs::read_to_string(&path)
                    .map_err(|e| format!("run-batch: cannot read input_file {path:?}: {e}"))?
            }
        };
        out.push(BatchWireItem {
            item_id,
            input,
            session_cwd: item.session_cwd,
        });
    }
    Ok(out)
}

struct RunBatchArgs {
    workflow: String,
    manifest: PathBuf,
    concurrency: Option<u32>,
    detach: bool,
    url: String,
}

fn parse_run_batch_args(args: &[String]) -> Result<RunBatchArgs, BoxError> {
    let mut positionals = Vec::new();
    let mut manifest = None;
    let mut concurrency = None;
    let mut detach = false;
    let mut url = "http://127.0.0.1:8080".to_string();
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--manifest" => {
                idx += 1;
                manifest = Some(PathBuf::from(
                    args.get(idx)
                        .ok_or("run-batch: --manifest requires a value")?,
                ));
                idx += 1;
            }
            "--concurrency" => {
                idx += 1;
                let raw = args
                    .get(idx)
                    .ok_or("run-batch: --concurrency requires a value")?;
                concurrency = Some(
                    raw.parse::<u32>()
                        .map_err(|e| format!("run-batch: invalid --concurrency {raw:?}: {e}"))?,
                );
                idx += 1;
            }
            "--detach" => {
                detach = true;
                idx += 1;
            }
            "--url" => {
                idx += 1;
                url = args
                    .get(idx)
                    .ok_or("run-batch: --url requires a value")?
                    .clone();
                idx += 1;
            }
            other if other.starts_with("--") => {
                return Err(format!("run-batch: unknown flag {other:?}\n{RUN_BATCH_USAGE}").into());
            }
            other => {
                positionals.push(other.to_string());
                idx += 1;
            }
        }
    }
    if positionals.len() != 1 {
        return Err(format!(
            "run-batch: expected exactly one <workflow>, got {}\n{RUN_BATCH_USAGE}",
            positionals.len()
        )
        .into());
    }
    let manifest = manifest
        .ok_or_else(|| format!("run-batch: --manifest <file> is required\n{RUN_BATCH_USAGE}"))?;
    Ok(RunBatchArgs {
        workflow: positionals.remove(0),
        manifest,
        concurrency,
        detach,
        url,
    })
}

fn rpc_result(v: serde_json::Value, label: &str) -> Result<serde_json::Value, BoxError> {
    if let Some(err) = v.get("error") {
        return Err(format!("{label} failed: {err}").into());
    }
    Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null))
}

fn batch_status_text(summary: &serde_json::Value) -> String {
    let status = summary["status"].as_str().unwrap_or("?");
    let running = summary["running"].as_u64().unwrap_or(0);
    let pending = summary["pending"].as_u64().unwrap_or(0);
    if status == "working" && running == 0 && pending == 0 {
        "settling…".to_string()
    } else {
        status.to_string()
    }
}

fn batch_rollup(summary: &serde_json::Value) -> String {
    format!(
        "{}  ok={} failed={} canceled={} running={} pending={}",
        batch_status_text(summary),
        summary["ok"].as_u64().unwrap_or(0),
        summary["failed"].as_u64().unwrap_or(0),
        summary["canceled"].as_u64().unwrap_or(0),
        summary["running"].as_u64().unwrap_or(0),
        summary["pending"].as_u64().unwrap_or(0)
    )
}

fn batch_is_terminal(summary: &serde_json::Value) -> bool {
    matches!(
        summary["status"].as_str(),
        Some("completed" | "failed" | "canceled")
    )
}

async fn batch_status(url: &str, id: &str) -> Result<serde_json::Value, BoxError> {
    let v = rpc_call(url, "BatchStatus", serde_json::json!({ "id": id })).await?;
    rpc_result(v, "batch status")
}

async fn poll_batch(url: &str, id: &str) -> Result<(), BoxError> {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
    loop {
        tokio::select! {
            _ = &mut ctrl_c => return Ok(()),
            _ = interval.tick() => {
                let summary = batch_status(url, id).await?;
                println!("{}", batch_rollup(&summary));
                if batch_is_terminal(&summary) {
                    return Ok(());
                }
            }
        }
    }
}

async fn run_batch_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{RUN_BATCH_USAGE}");
        return Ok(());
    }
    let parsed = parse_run_batch_args(args)?;
    let raw = std::fs::read_to_string(&parsed.manifest)
        .map_err(|e| format!("run-batch: cannot read manifest {:?}: {e}", parsed.manifest))?;
    let base = parsed
        .manifest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let items = parse_batch_manifest(&raw, base)?;
    let mut params = serde_json::json!({
        "workflow": parsed.workflow,
        "items": items,
    });
    if let Some(concurrency) = parsed.concurrency {
        params["concurrency"] = serde_json::json!(concurrency);
    }
    let result = rpc_result(
        rpc_call(&parsed.url, "RunBatch", params).await?,
        "run-batch",
    )?;
    let batch_id = result["batchId"]
        .as_str()
        .ok_or("run-batch: response missing result.batchId")?
        .to_string();
    println!("{batch_id}");
    if !parsed.detach {
        poll_batch(&parsed.url, &batch_id).await?;
    }
    Ok(())
}

async fn batch_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{BATCH_USAGE}");
        return Ok(());
    }
    let sub = args
        .first()
        .map(|s| s.as_str())
        .ok_or_else(|| format!("batch: missing subcommand (status|list|cancel)\n{BATCH_USAGE}"))?;
    let url = flag(args, "--url").unwrap_or("http://127.0.0.1:8080");
    match sub {
        "status" => {
            let id = args
                .get(1)
                .cloned()
                .ok_or("batch status: missing <batch-id>")?;
            let summary = batch_status(url, &id).await?;
            println!("{}", batch_rollup(&summary));
        }
        "list" => {
            let result = rpc_result(
                rpc_call(url, "BatchList", serde_json::json!({})).await?,
                "batch list",
            )?;
            for batch in result["batches"].as_array().cloned().unwrap_or_default() {
                println!(
                    "{}\t{}\t{}",
                    batch["id"].as_str().unwrap_or("?"),
                    batch["workflow"].as_str().unwrap_or("?"),
                    batch_rollup(&batch)
                );
            }
        }
        "cancel" => {
            let id = args
                .get(1)
                .cloned()
                .ok_or("batch cancel: missing <batch-id>")?;
            let result = rpc_result(
                rpc_call(url, "CancelBatch", serde_json::json!({ "id": id })).await?,
                "batch cancel",
            )?;
            println!("{}", result["canceled"].as_bool().unwrap_or(false));
        }
        other => return Err(format!("batch: unknown subcommand {other:?}\n{BATCH_USAGE}").into()),
    }
    Ok(())
}

async fn submit_cmd(args: &[String]) -> Result<(), BoxError> {
    let input_path = flag(args, "--input").ok_or("submit: --input <file> required")?;
    let url = flag(args, "--url").unwrap_or("http://127.0.0.1:8080");
    let text = std::fs::read_to_string(input_path)?;
    let mut md = serde_json::Map::new();
    let flagvals: std::collections::HashSet<&str> = [
        "--input",
        "--url",
        "--context",
        "--agent",
        "--model",
        "--effort",
        "--mode",
        "--cwd",
    ]
    .iter()
    .filter_map(|f| flag(args, f))
    .collect();
    let skill = args
        .iter()
        .find(|a| !a.starts_with("--") && !flagvals.contains(a.as_str()))
        .cloned();
    if let Some(s) = &skill {
        md.insert("a2a-bridge.skill".into(), s.clone().into());
    }
    for (f, key) in [
        ("--agent", "a2a-bridge.agent"),
        ("--model", "a2a-bridge.model"),
        ("--effort", "a2a-bridge.effort"),
        ("--mode", "a2a-bridge.mode"),
        ("--cwd", "a2a-bridge.cwd"),
    ] {
        if let Some(v) = flag(args, f) {
            md.insert(key.into(), v.into());
        }
    }
    let mut message = serde_json::Map::new();
    message.insert("text".into(), text.into());
    message.insert("metadata".into(), serde_json::Value::Object(md));
    if let Some(c) = flag(args, "--context") {
        message.insert("contextId".into(), c.into());
    }
    let v = rpc_call(
        url,
        a2a::methods::SEND_MESSAGE,
        serde_json::json!({ "message": message }),
    )
    .await?;
    if let Some(err) = v.get("error") {
        return Err(format!("submit failed: {err}").into());
    }
    let out = v["result"]["artifact"]["text"]
        .as_str()
        .or_else(|| {
            v["result"]["artifacts"]
                .as_array()
                .and_then(|artifacts| artifacts.iter().find_map(artifact_text))
        })
        .or_else(|| v["result"]["task"]["id"].as_str())
        .unwrap_or("ok");
    println!("{out}");
    Ok(())
}

fn artifact_text(artifact: &serde_json::Value) -> Option<&str> {
    artifact["parts"]
        .as_array()
        .and_then(|parts| parts.iter().find_map(|part| part["text"].as_str()))
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

async fn session_cmd(args: &[String]) -> Result<(), BoxError> {
    let sub = args
        .first()
        .map(|s| s.as_str())
        .ok_or("session: missing subcommand (status|release|cancel|clear|compact|inject|permit)")?;
    let url = flag(args, "--url").unwrap_or("http://127.0.0.1:8080");
    let (method, params) = build_session_rpc(args)?;
    let v = rpc_call(url, method, params).await?;
    if let Some(err) = v.get("error") {
        return Err(format!("session {sub} failed: {err}").into());
    }
    println!("{}", serde_json::to_string_pretty(&v["result"])?);
    Ok(())
}

fn build_session_rpc(args: &[String]) -> Result<(&'static str, serde_json::Value), BoxError> {
    let sub = args
        .first()
        .map(|s| s.as_str())
        .ok_or("session: missing subcommand (status|release|cancel|clear|compact|inject|permit)")?;
    match sub {
        "status" | "release" | "cancel" | "clear" | "compact" => {
            let ctx = args.get(1).cloned().ok_or("session: missing <contextId>")?;
            let method = match sub {
                "status" => "SessionStatus",
                "release" => "SessionRelease",
                "cancel" => "SessionCancel",
                "clear" => "SessionClear",
                "compact" => "SessionCompact",
                _ => unreachable!(),
            };
            let force = args.iter().any(|a| a == "--force");
            let params = if sub == "clear" {
                serde_json::json!({ "contextId": ctx, "force": force })
            } else {
                serde_json::json!({ "contextId": ctx })
            };
            Ok((method, params))
        }
        "inject" => build_session_inject_rpc(args),
        "permit" => build_session_permit_rpc(args),
        other => Err(format!("session: unknown subcommand {other:?}").into()),
    }
}

fn build_session_inject_rpc(
    args: &[String],
) -> Result<(&'static str, serde_json::Value), BoxError> {
    let ctx = args
        .get(1)
        .cloned()
        .ok_or("session inject: missing <contextId>")?;
    let input_path = flag(args, "--input").ok_or("session inject: --input <file> required")?;
    let text = std::fs::read_to_string(input_path)?;
    let mut params = serde_json::json!({
        "contextId": ctx,
        "text": text,
        "mode": if args.iter().any(|a| a == "--append") {
            "append_next_turn"
        } else {
            "prepend_next_turn"
        }
    });
    if let Some(dedupe) = flag(args, "--dedupe") {
        params["dedupeKey"] = serde_json::Value::String(dedupe.to_string());
    }
    Ok(("SessionInject", params))
}

fn build_session_permit_rpc(
    args: &[String],
) -> Result<(&'static str, serde_json::Value), BoxError> {
    let request_id = args
        .get(1)
        .cloned()
        .ok_or("session permit: missing <requestId>")?;
    let context =
        flag(args, "--context").ok_or("session permit: --context <contextId> required")?;
    let generation: u64 = flag(args, "--generation")
        .ok_or("session permit: --generation <n> required")?
        .parse()
        .map_err(|_| "session permit: invalid --generation")?;
    let op = flag(args, "--op").ok_or("session permit: --op <operationId> required")?;
    let mut selected = Vec::new();
    if args.iter().any(|a| a == "--approve") {
        selected.push("approve");
    }
    if args.iter().any(|a| a == "--deny") {
        selected.push("deny");
    }
    if flag(args, "--modify").is_some() {
        selected.push("modify");
    }
    if args.iter().any(|a| a == "--escalate") {
        selected.push("escalate");
    }
    if selected.len() != 1 {
        return Err(
            "session permit: choose exactly one of --approve|--deny|--modify <id>|--escalate"
                .into(),
        );
    }

    let decision = match selected[0] {
        "approve" => {
            let mut d = serde_json::json!({ "decision": "approve" });
            if let Some(option) = flag(args, "--option") {
                d["optionId"] = serde_json::Value::String(option.to_string());
            }
            d
        }
        "deny" => {
            let mut d = serde_json::json!({ "decision": "deny" });
            if let Some(option) = flag(args, "--option") {
                d["optionId"] = serde_json::Value::String(option.to_string());
            }
            if let Some(reason) = flag(args, "--reason") {
                d["reason"] = serde_json::Value::String(reason.to_string());
            }
            d
        }
        "modify" => serde_json::json!({
            "decision": "modify",
            "optionId": flag(args, "--modify").expect("selected modify has a value")
        }),
        "escalate" => {
            let mut d = serde_json::json!({ "decision": "escalate" });
            if let Some(reason) = flag(args, "--reason") {
                d["reason"] = serde_json::Value::String(reason.to_string());
            }
            d
        }
        _ => unreachable!(),
    };

    Ok((
        "SessionPermit",
        serde_json::json!({
            "contextId": context,
            "generation": generation,
            "op": op,
            "requestId": request_id,
            "decision": decision
        }),
    ))
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

async fn mcp_cmd(args: &[String]) -> Result<(), BoxError> {
    bridge_observ::init_stderr();

    let mut explicit_config: Option<PathBuf> = None;
    let mut store_override: Option<PathBuf> = None;
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--help" | "-h" => {
                println!("{MCP_USAGE}");
                return Ok(());
            }
            "--config" => {
                explicit_config = Some(PathBuf::from(
                    iter.next().ok_or("mcp: --config requires a <path>")?,
                ));
            }
            "--store" => {
                store_override = Some(PathBuf::from(
                    iter.next().ok_or("mcp: --store requires a <path>")?,
                ));
            }
            other => {
                return Err(format!("mcp: unknown flag {other:?}\n{MCP_USAGE}").into());
            }
        }
    }

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

    let host = bridge_core::liveness::host_id();
    let instance_id = format!("{}-{}", std::process::id(), implement::nonce(8));
    let lease = bridge_core::liveness::acquire_lease(&instance_id)
        .map_err(|e| format!("mcp: acquire run lease: {e}"))?;
    let run = bridge_core::run_identity::RunHandle {
        instance_id,
        host: host.clone(),
        lease: lease.path().to_string_lossy().to_string(),
        start: epoch_secs(),
    };

    let raw = std::fs::read_to_string(&config_path)?;
    let cfg = RegistryConfig::parse(&raw)?;
    let worktree_cfg = resolve_worktree_runtime_cfg(&cfg).map_err(|e| format!("mcp: {e}"))?;

    let policy = make_policy(&cfg.server);
    let perm_registry = PermissionRegistry::new();
    let perm_timeout = permission_timeout_ms(&cfg.server);
    let spawn: SpawnFn = make_spawn_fn(
        Arc::clone(&policy) as Arc<dyn PolicyEngine>,
        config_path.clone(),
        run.clone(),
        Some(Arc::clone(&perm_registry)),
        perm_timeout,
        worktree_cfg.clone(),
    );

    let source = FileConfigSource::new(config_path.clone());
    let snapshot = source.load().await?;
    recover_orphans(&snapshot, &config_path, &host);
    if let Some(wc) = &worktree_cfg {
        bridge_worktree::sweep::sweep_orphans(
            &wc.root,
            &host,
            &bridge_core::liveness::FsLeaseProbe,
        );
    }
    let registry = Arc::new(Registry::new(snapshot, spawn)?);

    let base = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let wf_map = cfg.load_workflows(base)?;
    let executor = Arc::new(bridge_workflow::executor::WorkflowExecutor::new(
        Arc::clone(&registry) as _,
    ));

    let clock: Arc<dyn bridge_coordinator::clock::Clock> =
        Arc::new(bridge_coordinator::clock::SystemClock);
    let warm_ttl = cfg.server.warm_idle_ttl_secs;
    let registry_for_sessions: Arc<dyn AgentRegistry> = registry.clone();
    let session_manager = Arc::new(
        bridge_coordinator::session_manager::SessionManager::new_with_clock(
            registry_for_sessions,
            Duration::from_secs(warm_ttl),
            clock.clone(),
        )
        .with_permission_registry(Arc::clone(&perm_registry))
        .with_warn_fraction(cfg.server.warm_usage_warn_fraction)
        .with_compact_summarize_timeout(Duration::from_secs(
            cfg.server.compact_summarize_timeout_secs.unwrap_or(120),
        )),
    );
    {
        let sm = session_manager.clone();
        let period = Duration::from_secs(warm_ttl.clamp(1, 30));
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(period);
            loop {
                ticker.tick().await;
                sm.reap_idle().await;
            }
        });
    }

    let resume_cap = cfg
        .store
        .as_ref()
        .and_then(|s| s.resume_attempt_cap)
        .unwrap_or(3);
    let resolve_rel = |p: &std::path::Path| {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            base.join(p)
        }
    };
    let store_path: Option<PathBuf> = match (&store_override, cfg.store.as_ref()) {
        (Some(p), _) => Some(resolve_rel(p)),
        (None, Some(s)) => Some(resolve_rel(std::path::Path::new(&s.path))),
        (None, None) => None,
    };
    let (session_store, task_store): (
        Arc<dyn bridge_core::ports::SessionStore>,
        Arc<dyn bridge_core::task_store::TaskStore>,
    ) = match store_path {
        Some(path) => {
            let sqlite = Arc::new(SqliteStore::open(&path).map_err(|e| {
                format!(
                    "a2a-bridge mcp: cannot open task store {path:?} ({e:?}); \
                     is another a2a-bridge serve/mcp already running on this store?"
                )
            })?);
            (sqlite.clone() as _, sqlite as _)
        }
        None => {
            let sqlite = Arc::new(SqliteStore::open_in_memory()?);
            (sqlite.clone() as _, sqlite as _)
        }
    };

    let allowed_cwd_root = cfg
        .allowed_cwd_root
        .as_deref()
        .map(bridge_core::session_cwd::SessionCwd::parse)
        .transpose()
        .map_err(|e| format!("a2a-bridge mcp: invalid allowed_cwd_root: {e:?}"))?;
    let batch = batch_runtime(&cfg).map_err(|e| format!("a2a-bridge mcp: {e}"))?;

    let coordinator = Arc::new(
        bridge_coordinator::Coordinator::new(
            session_manager,
            Some(executor),
            Arc::new(wf_map),
            task_store,
            session_store,
            Arc::clone(&policy) as Arc<dyn PolicyEngine>,
            Arc::clone(&registry) as Arc<dyn AgentRegistry>,
            clock,
            allowed_cwd_root,
            batch,
            resume_cap,
        )
        .with_permission_registry(Arc::clone(&perm_registry)),
    );

    coordinator.resume().await;
    bridge_mcp::serve(tokio::io::stdin(), tokio::io::stdout(), coordinator).await?;
    Ok(())
}

/// Read a task input from a file path, or stdin when `path` is "-". Returns the raw String.
fn read_input(path: &str) -> Result<String, BoxError> {
    if path == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        Ok(s)
    } else {
        Ok(std::fs::read_to_string(path)?)
    }
}

fn task_spec_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{TASK_SPEC_USAGE}");
        return Ok(());
    }

    let Some(sub) = args.first().map(|s| s.as_str()) else {
        return Err(format!("task-spec: missing subcommand\n{TASK_SPEC_USAGE}").into());
    };

    match sub {
        "schema" => {
            if args.len() > 2 {
                return Err(
                    format!("task-spec schema: too many arguments\n{TASK_SPEC_USAGE}").into(),
                );
            }

            if let Some(task_type) = args.get(1) {
                let schema = bridge_core::task_spec::schema(task_type).ok_or_else(|| {
                    bridge_core::task_spec::TaskSpecError::UnknownType {
                        got: task_type.clone(),
                    }
                    .to_string()
                })?;
                println!("{}: {}", schema.task_type, schema.summary);
                for section in schema.sections {
                    let requirement = if section.required {
                        "REQUIRED"
                    } else {
                        "OPTIONAL"
                    };
                    println!("{} [{}] {}", section.name, requirement, section.description);
                }
            } else {
                for task_type in bridge_core::task_spec::task_types() {
                    if let Some(schema) = bridge_core::task_spec::schema(task_type) {
                        println!("{}: {}", schema.task_type, schema.summary);
                    }
                }
            }
            Ok(())
        }
        "template" => {
            if args.len() != 2 {
                return Err(
                    format!("task-spec template: expected <type>\n{TASK_SPEC_USAGE}").into(),
                );
            }
            let task_type = &args[1];
            let template = bridge_core::task_spec::template(task_type).ok_or_else(|| {
                bridge_core::task_spec::TaskSpecError::UnknownType {
                    got: task_type.clone(),
                }
                .to_string()
            })?;
            print!("{template}");
            Ok(())
        }
        "input" => {
            if args.len() != 2 {
                return Err(
                    format!("task-spec input: expected <file|->\n{TASK_SPEC_USAGE}").into(),
                );
            }
            let raw = read_input(&args[1])?;
            match bridge_core::task_spec::validate_input(&raw) {
                Ok(spec) => {
                    let body = bridge_core::task_spec::body(&spec);
                    print!("{body}");
                    if !body.ends_with('\n') {
                        println!();
                    }
                    let keys: Vec<String> = bridge_core::task_spec::fields(&spec)
                        .into_iter()
                        .map(|(key, _)| key)
                        .collect();
                    println!("fields: {}", keys.join(", "));
                    Ok(())
                }
                Err(e) => {
                    eprintln!("{}", e.client_message());
                    Err(e.into())
                }
            }
        }
        other => Err(format!("task-spec: unknown subcommand {other:?}\n{TASK_SPEC_USAGE}").into()),
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    // Dispatch subcommands BEFORE the server path touches the filesystem.
    let raw_args: Vec<String> = std::env::args().collect();
    match parse_top_subcommand(&raw_args) {
        TopSubcommand::RunWorkflow => return run_workflow_cmd(&raw_args[2..]).await,
        TopSubcommand::RunBatch => return run_batch_cmd(&raw_args[2..]).await,
        TopSubcommand::Batch => return batch_cmd(&raw_args[2..]).await,
        TopSubcommand::Models => return models_cmd(&raw_args[2..]).await,
        TopSubcommand::Implement => return implement_cmd(&raw_args[2..]).await,
        TopSubcommand::Merge => return merge::merge_cmd(&raw_args[2..]).await,
        TopSubcommand::Containers => return containers_cmd(&raw_args[2..]),
        TopSubcommand::Submit => return submit_cmd(&raw_args[2..]).await,
        TopSubcommand::Task => return task_cmd(&raw_args[2..]).await,
        TopSubcommand::Session => return session_cmd(&raw_args[2..]).await,
        TopSubcommand::Init => return init_cmd(&raw_args[2..]),
        TopSubcommand::Mcp => return mcp_cmd(&raw_args[2..]).await,
        TopSubcommand::TaskSpec => return task_spec_cmd(&raw_args[2..]),
        TopSubcommand::Help => {
            println!("{TOP_USAGE}");
            return Ok(());
        }
        // `serve` (explicit) and the bare invocation fall through to the server path.
        TopSubcommand::Serve => {}
        // An unknown first token must NOT silently serve (a typo'd subcommand or flag
        // would otherwise be swallowed and the default served).
        TopSubcommand::Unknown(other) => {
            return Err(format!(
                "a2a-bridge: unknown subcommand {other:?} (expected: serve | mcp | run-workflow | run-batch | batch | models | implement | merge | containers | submit | task | task-spec | session | init | help)"
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

    // Read the non-registry config sections (server addr, delegation) directly:
    // the FileConfigSource snapshot only carries the registry. Re-parsing the same
    // file is cheap and keeps the [server]/[delegation] parsing (incl. env-expansion)
    // working on the RegistryConfig path.
    let raw = std::fs::read_to_string(&config_path)?;
    let cfg = RegistryConfig::parse(&raw)?;
    let worktree_cfg = resolve_worktree_runtime_cfg(&cfg).map_err(|e| format!("serve: {e}"))?;

    // 3. Build the policy engine FIRST so the SAME engine drives both the inbound
    //    server's permission decisions AND each backend's REVERSE
    //    `session/request_permission` decisions (threaded via `with_policy`), so
    //    the system applies one consistent permission policy in both directions.
    let policy = make_policy(&cfg.server);
    let perm_registry = PermissionRegistry::new();
    let perm_timeout = permission_timeout_ms(&cfg.server);

    // 4. SpawnFn — the registry's adapter factory. Lazily spawns a real AcpBackend
    //    per entry: it runs initialize → authenticate, owns the `Supervised` child
    //    for the backend's lifetime, and applies the configured mode/model after
    //    each `session/new`. `model`/`mode` here are the per-MINT FALLBACK; the
    //    per-session `configure_session` overrides them at dispatch (Task 6).
    let spawn: SpawnFn = make_spawn_fn(
        Arc::clone(&policy) as Arc<dyn PolicyEngine>,
        config_path.clone(),
        run.clone(),
        Some(Arc::clone(&perm_registry)),
        perm_timeout,
        worktree_cfg.clone(),
    );

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
    if let Some(wc) = &worktree_cfg {
        bridge_worktree::sweep::sweep_orphans(
            &wc.root,
            &host,
            &bridge_core::liveness::FsLeaseProbe,
        );
    }
    // Fan-out source label (wire-observable in fan-out artifacts): the default
    // entry's `name` if set, else the default agent id, so a non-Kiro default
    // (e.g. codex) isn't mislabeled "kiro".
    let default_label = snapshot
        .entries
        .iter()
        .find(|e| e.id == snapshot.default)
        .and_then(|e| e.name.clone())
        .unwrap_or_else(|| snapshot.default.as_str().to_string());
    // advertise-models: capture (id, entry) pairs BEFORE the snapshot moves into `Registry::new`, so the
    // startup probe + SIGHUP re-probe can build the model catalog host-side (sandbox-independent; spec §2).
    let probe_entries: Vec<(String, AgentEntry)> = snapshot
        .entries
        .iter()
        .map(|e| (e.id.as_str().to_string(), e.clone()))
        .collect();
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

    // 7b. advertise-models: probe each agent's advertised model/effort/mode catalog HOST-SIDE before the
    //     first card is served (bounded + degrade-per-agent; spec §5). Held behind an `ArcSwap` the card
    //     reads lock-free; a `SIGHUP` handler (below) re-probes and atomically swaps it. The advertised
    //     list is account/adapter-driven and sandbox-independent, so this never spins a container.
    let probe_cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
    let model_catalog = Arc::new(arc_swap::ArcSwap::from_pointee(
        catalog_probe::probe_all(&probe_entries, &probe_cwd).await,
    ));

    // Slice 0 warm sessions: share the live registry with the SessionManager and reap idle handles.
    let warm_ttl = cfg.server.warm_idle_ttl_secs;
    let registry_for_sessions: Arc<dyn AgentRegistry> = registry.clone();
    let session_manager = Arc::new(
        bridge_a2a_inbound::session_manager::SessionManager::new(
            registry_for_sessions,
            Duration::from_secs(warm_ttl),
        )
        .with_permission_registry(Arc::clone(&perm_registry))
        .with_warn_fraction(cfg.server.warm_usage_warn_fraction)
        .with_compact_summarize_timeout(Duration::from_secs(
            cfg.server.compact_summarize_timeout_secs.unwrap_or(120),
        )),
    );
    {
        let sm = session_manager.clone();
        let period = Duration::from_secs(warm_ttl.clamp(1, 30));
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(period);
            loop {
                ticker.tick().await;
                sm.reap_idle().await;
            }
        });
    }

    // 8. Construct the inbound server.
    //    InboundServer::new(registry, store, policy, route, auth, base_url, delegation, local_source_label)
    // The inbound server holds the agent registry (3b): first-message LOCAL dispatch
    // resolves the routed agent id, applies its effective config, and binds the task.
    let base_url = format!("http://{}", cfg.server.addr);
    let batch = batch_runtime(&cfg)?;
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
        .with_session_manager(session_manager)
        .with_permission_registry(Arc::clone(&perm_registry))
        .with_allowed_cwd_root(cfg.allowed_cwd_root.clone())
        .with_batch_runtime(batch)
        .with_model_catalog(Arc::clone(&model_catalog)),
    );

    // 8a. SIGHUP → re-probe the model catalog and atomically swap it (no background timer; owner decision).
    //     The card path reads the new catalog on its next request; in-flight requests are never dropped.
    {
        let sighup_catalog = Arc::clone(&model_catalog);
        let sighup_entries = probe_entries.clone();
        let sighup_cwd = probe_cwd.clone();
        tokio::spawn(async move {
            let mut hup = match tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::hangup(),
            ) {
                Ok(sig) => sig,
                Err(e) => {
                    tracing::warn!(error = %e, "SIGHUP handler unavailable; model catalog will not hot-refresh");
                    return;
                }
            };
            while hup.recv().await.is_some() {
                tracing::info!("SIGHUP: re-probing model catalog");
                let fresh = catalog_probe::probe_all(&sighup_entries, &sighup_cwd).await;
                sighup_catalog.store(Arc::new(fresh));
            }
        });
    }

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

    #[test]
    fn make_policy_selects_defer_and_timeout_default() {
        let cfg = RegistryConfig::parse(
            "default=\"k\"\n[[agents]]\nid=\"k\"\ncmd=\"k\"\n[server]\npermission_policy=\"defer\"\npermission_timeout_ms=5000\n",
        )
        .unwrap();
        assert!(matches!(
            make_policy(&cfg.server).interactive_decide(
                &bridge_core::domain::PermissionRequest::interactive(),
                &bridge_core::domain::SessionContext::test(),
            ),
            bridge_core::ports::PolicyOutcome::Defer
        ));
        assert_eq!(permission_timeout_ms(&cfg.server), 5000);

        let cfg =
            RegistryConfig::parse("default=\"k\"\n[[agents]]\nid=\"k\"\ncmd=\"k\"\n[server]\n")
                .unwrap();
        assert!(matches!(
            make_policy(&cfg.server).interactive_decide(
                &bridge_core::domain::PermissionRequest::read(),
                &bridge_core::domain::SessionContext::test(),
            ),
            bridge_core::ports::PolicyOutcome::Decide(Ok(
                bridge_core::domain::PermissionDecision::Approve
            ))
        ));
        assert_eq!(permission_timeout_ms(&cfg.server), 120_000);
    }

    #[test]
    fn session_inject_parser_builds_append_payload() {
        let path =
            std::env::temp_dir().join(format!("a2a-bridge-inject-{}.txt", std::process::id()));
        std::fs::write(&path, "queued text").unwrap();
        let args = vec![
            "inject".to_string(),
            "ctx-cli".to_string(),
            "--input".to_string(),
            path.to_string_lossy().to_string(),
            "--append".to_string(),
            "--dedupe".to_string(),
            "k1".to_string(),
        ];

        let (method, params) = build_session_rpc(&args).unwrap();

        let _ = std::fs::remove_file(&path);
        assert_eq!(method, "SessionInject");
        assert_eq!(params["contextId"], "ctx-cli");
        assert_eq!(params["text"], "queued text");
        assert_eq!(params["mode"], "append_next_turn");
        assert_eq!(params["dedupeKey"], "k1");
    }

    #[test]
    fn session_permit_parser_builds_approve_payload() {
        let args = vec![
            "permit".to_string(),
            "req-cli".to_string(),
            "--context".to_string(),
            "ctx-cli".to_string(),
            "--generation".to_string(),
            "4".to_string(),
            "--op".to_string(),
            "turn-4".to_string(),
            "--approve".to_string(),
            "--option".to_string(),
            "approved".to_string(),
        ];

        let (method, params) = build_session_rpc(&args).unwrap();

        assert_eq!(method, "SessionPermit");
        assert_eq!(params["contextId"], "ctx-cli");
        assert_eq!(params["generation"], 4);
        assert_eq!(params["op"], "turn-4");
        assert_eq!(params["requestId"], "req-cli");
        assert_eq!(
            params["decision"],
            serde_json::json!({ "decision": "approve", "optionId": "approved" })
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
                retry: None,
            })
            .collect();
        WorkflowGraph {
            id: WorkflowId::parse(id).unwrap(),
            nodes,
            panel: None,
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

    #[test]
    fn acp_spawn_inputs_forwards_watchdog() {
        let mut entry = acp_entry("reader");
        entry.watchdog = Some(bridge_core::domain::WatchdogConfig {
            idle_timeout: std::time::Duration::from_secs(30),
            hard_wall_clock: std::time::Duration::from_secs(600),
        });
        let run = bridge_core::run_identity::RunHandle {
            instance_id: "run0".into(),
            host: "h".into(),
            lease: "/l/run0.lock".into(),
            start: "0".into(),
        };

        let (_, _, acp) = acp_spawn_inputs(
            &entry,
            std::path::PathBuf::from("/tmp"),
            std::path::Path::new("/cfg/a2a.toml"),
            &run,
        )
        .unwrap();

        let wd = acp.watchdog.as_ref().expect("watchdog is forwarded");
        assert_eq!(wd.idle_timeout, std::time::Duration::from_secs(30));
        assert_eq!(wd.hard_wall_clock, std::time::Duration::from_secs(600));
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
            watchdog: None,
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

    /// FU1 codex `{cwd}` CONTRACT TEST (Slice C1 D2 deterministic fork). The per-request `--session-cwd`
    /// is stamped into `entry.session_cwd` (run-workflow ~L2081 / serve per-request) BEFORE the SpawnFn
    /// runs. This LOCKS that the spawn-site resolution chain threads that stamped value through to codex's
    /// native `-c mcp_servers.*` `{cwd}` substitution — NOT the bridge launch dir. Replays the EXACT chain
    /// the spawn site runs: resolve_static_session_cwd → absolutize/passthrough → canonicalize-or-raw
    /// (`acp_spawn_inputs`' `mcp_cwd`) → `acp_program_argv`. NOTE: deterministic source-level contract;
    /// the live `{cwd}` gate (Phase B) is the final arbiter — the historical live break may be unreadable
    /// from source, so a green here does NOT prove live behavior.
    #[test]
    fn codex_mcp_cwd_uses_stamped_session_cwd() {
        use bridge_core::mcp::{McpDelivery, McpServerSpec};
        // A real per-request Python repo dir so the spawn-site canonicalize() succeeds (mirrors prod).
        let repo = std::env::temp_dir().join(format!("a2a-fu1-pyrepo-{}", implement::nonce(8)));
        std::fs::create_dir_all(&repo).unwrap();
        let repo_str = repo.to_string_lossy().to_string();
        // canonicalize the expected value the same way `acp_spawn_inputs` computes `mcp_cwd` (on macOS
        // /tmp → /private/tmp), so the assertion matches what codex actually receives.
        let expected = std::fs::canonicalize(&repo)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| repo_str.clone());

        // A codex CodexNative agent whose mcp server args carry the `{cwd}` placeholder.
        let mut codex = acp_entry("codex");
        codex.cmd = Some("codex-acp".into());
        codex.mcp_delivery = McpDelivery::CodexNative;
        codex.mcp = vec![McpServerSpec {
            name: "lsp".into(),
            command: "/opt/lsp-mcp".into(),
            args: vec![
                "--repo".into(),
                "{cwd}".into(),
                "--lang".into(),
                "auto".into(),
            ],
            env: vec![],
        }];
        // The per-request stamp the run-workflow/serve path applies BEFORE the SpawnFn runs.
        codex.session_cwd = Some(repo_str.clone());

        // Replay the EXACT spawn-site resolution chain (main.rs ~L470-471 → acp_spawn_inputs ~L201).
        let resolved =
            resolve_static_session_cwd(codex.session_cwd.as_deref(), codex.cwd.as_deref());
        let abs = {
            let p = PathBuf::from(&resolved);
            if p.is_absolute() {
                p
            } else {
                std::env::current_dir().unwrap().join(p)
            }
        };
        let mcp_cwd = std::fs::canonicalize(&abs)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| abs.to_string_lossy().into_owned());

        let (_p, argv) = acp_program_argv(&codex, None, &[], &mcp_cwd).unwrap();

        // The codex argv must carry the {cwd}-substituted STAMPED session_cwd (the python repo),
        // not the launch dir, and no literal {cwd} must survive.
        assert!(
            argv.iter().any(|a| a.contains(&expected)),
            "codex argv must contain stamped session_cwd {expected:?}: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.contains("{cwd}")),
            "no literal {{cwd}} may survive: {argv:?}"
        );
        let launch = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(
            !argv.iter().any(|a| a.contains(&launch)),
            "codex argv must NOT contain the launch dir {launch:?}: {argv:?}"
        );

        std::fs::remove_dir_all(&repo).ok();
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
[[languages]]
id = "rust"
fetch = "cargo fetch --locked"
warm_cache = "a2a-impl-lsp-cache"
dep_cache_path = "/cargo"
verify_cache_path = "/cache"
[[languages.verify]]
name = "t"
cmd = "true"
"#;
        let cfg = config::RegistryConfig::parse(toml).expect("parses");
        let verify_cfg = cfg.verify.as_ref().map(|t| t.to_config());
        let profile = select_profile(
            &cfg,
            &LangArg::Explicit("rust".into()),
            std::path::Path::new("/"),
        )
        .expect("profile");
        let snapshot = cfg.into_snapshot().expect("snapshots");
        assert!(
            snapshot.allowed_cmds.iter().any(|c| c == "podman")
                && !snapshot.allowed_cmds.iter().any(|c| c == "docker"),
            "default union is podman-only"
        );
        let gated = config::gate_verify_runtime(verify_cfg, &snapshot.allowed_cmds);
        let outcome = run_verify_step(
            &gated,
            profile.as_ref(),
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

    #[test]
    fn manifest_parse_defaults_id_dedups_and_xor_input() {
        let base = std::path::Path::new(".");
        let items = super::parse_batch_manifest(
            r#"
[[item]]
input = "one"
"#,
            base,
        )
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_id, "0");
        assert_eq!(items[0].input, "one");

        assert!(super::parse_batch_manifest(
            r#"
[[item]]
id = "same"
input = "one"

[[item]]
id = "same"
input = "two"
"#,
            base,
        )
        .is_err());

        assert!(super::parse_batch_manifest(
            r#"
[[item]]
input = "one"
input_file = "one.md"
"#,
            base,
        )
        .is_err());

        assert!(super::parse_batch_manifest(
            r#"
[[item]]
id = "empty"
"#,
            base,
        )
        .is_err());

        assert!(super::parse_batch_manifest("", base).is_err());
    }

    #[test]
    fn task_spec_subcommand_is_registered() {
        let raw_args: Vec<String> = ["a2a-bridge", "task-spec", "schema"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            super::parse_top_subcommand(&raw_args),
            super::TopSubcommand::TaskSpec
        );

        assert!(super::task_spec_cmd(&["schema".to_string()]).is_ok());
        assert!(super::task_spec_cmd(&["template".to_string(), "implement".to_string()]).is_ok());

        let path = temp_task_spec_path("task-spec-registered");
        std::fs::write(&path, valid_implement_task_spec()).unwrap();
        assert!(
            super::task_spec_cmd(&["input".to_string(), path.to_string_lossy().to_string(),])
                .is_ok()
        );
        let _ = std::fs::remove_file(&path);

        assert!(super::task_spec_cmd(&["bogus".to_string()]).is_err());
    }

    #[test]
    fn read_input_file() {
        let path = temp_task_spec_path("read-input-file");
        std::fs::write(&path, "raw file contents\n").unwrap();
        let raw = super::read_input(&path.to_string_lossy()).unwrap();
        assert_eq!(raw, "raw file contents\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn task_spec_template_round_trips() {
        let t = bridge_core::task_spec::template("implement").unwrap();
        assert!(bridge_core::task_spec::parse(&t).is_ok());
        assert!(bridge_core::task_spec::task_types().contains(&"implement"));
    }

    fn temp_task_spec_path(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("a2a-{label}-{}-{nanos}.md", std::process::id()))
    }

    fn valid_implement_task_spec() -> &'static str {
        "\
---
task-type: implement
---
# Add task-spec CLI

## Description
Add the task-spec command.

## Acceptance Criteria
The command prints schemas, templates, and validates input.
"
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
        let (id, input, _out, _cfg, scwd, serve, _url, context) =
            super::parse_run_workflow_args(&args).unwrap();
        assert_eq!(id, "wf");
        assert_eq!(input, std::path::PathBuf::from("in.md"));
        assert_eq!(scwd.as_deref(), Some("/work/repo"));
        assert!(!serve);
        assert!(context.is_none());
    }

    #[test]
    fn parse_run_workflow_args_no_session_cwd_is_none() {
        let args: Vec<String> = ["wf", "--input", "in.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (_id, _input, _out, _cfg, scwd, _serve, _url, _context) =
            super::parse_run_workflow_args(&args).unwrap();
        assert!(scwd.is_none());
    }

    #[test]
    fn run_workflow_context_requires_serve() {
        let args: Vec<String> = ["wf", "--input", "in.md", "--context", "C"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let err = super::parse_run_workflow_args(&args)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--context requires --serve"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn run_workflow_url_requires_serve() {
        let args: Vec<String> = ["wf", "--input", "in.md", "--url", "http://127.0.0.1:9090"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let err = super::parse_run_workflow_args(&args)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--url requires --serve"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn run_workflow_config_rejected_with_serve() {
        let args: Vec<String> = [
            "--serve",
            "--context",
            "C",
            "--config",
            "local.toml",
            "wf",
            "--input",
            "in.md",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let err = super::parse_run_workflow_args(&args)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--config cannot be used with --serve"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn run_workflow_serve_flags_before_workflow_id() {
        let args: Vec<String> = ["--serve", "--context", "C", "wf", "--input", "in.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (id, input, _out, _cfg, scwd, serve, url, context) =
            super::parse_run_workflow_args(&args).unwrap();
        assert_eq!(id, "wf");
        assert_eq!(input, std::path::PathBuf::from("in.md"));
        assert!(scwd.is_none());
        assert!(serve);
        assert_eq!(url, "http://127.0.0.1:8080");
        assert_eq!(context.as_deref(), Some("C"));
    }

    #[test]
    fn run_workflow_serve_requires_context() {
        let args: Vec<String> = ["--serve", "wf", "--input", "in.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let err = super::parse_run_workflow_args(&args)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("--serve requires --context"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn serve_client_builds_streaming_message() {
        let req =
            super::build_run_workflow_streaming_request("wf", "hello", "C", Some("/work/repo"));
        assert_eq!(req["method"], a2a::methods::SEND_STREAMING_MESSAGE);
        let message = &req["params"]["message"];
        assert_eq!(message["contextId"], "C");
        assert!(
            message["taskId"].as_str().is_some_and(|s| !s.is_empty()),
            "taskId missing from {req}"
        );
        assert_eq!(message["metadata"]["a2a-bridge.skill"], "wf");
        assert_eq!(message["metadata"]["a2a-bridge.cwd"], "/work/repo");
        assert_eq!(message["parts"][0]["kind"], "text");
        assert_eq!(message["parts"][0]["text"], "hello");
    }

    #[test]
    fn serve_client_requests_have_distinct_task_ids() {
        let first = super::build_run_workflow_streaming_request("wf", "hello", "C", None);
        let second = super::build_run_workflow_streaming_request("wf", "hello", "C", None);
        assert_ne!(
            first["params"]["message"]["taskId"],
            second["params"]["message"]["taskId"]
        );
    }

    fn stream_response_sse(resp: a2a::StreamResponse) -> String {
        format!("data: {}\n\n", serde_json::to_string(&resp).unwrap())
    }

    fn artifact_update(text: &str) -> a2a::StreamResponse {
        a2a::StreamResponse::ArtifactUpdate(a2a::TaskArtifactUpdateEvent {
            task_id: "task-1".to_string(),
            context_id: "C".to_string(),
            artifact: a2a::Artifact {
                artifact_id: a2a::new_artifact_id(),
                name: Some("output".to_string()),
                description: None,
                parts: vec![a2a::Part::text(text)],
                metadata: None,
                extensions: None,
            },
            append: None,
            last_chunk: Some(true),
            metadata: None,
        })
    }

    fn terminal_update(state: a2a::TaskState) -> a2a::StreamResponse {
        a2a::StreamResponse::StatusUpdate(a2a::TaskStatusUpdateEvent {
            task_id: "task-1".to_string(),
            context_id: "C".to_string(),
            status: a2a::TaskStatus {
                state,
                message: None,
                timestamp: None,
            },
            metadata: None,
        })
    }

    async fn run_workflow_with_fake_serve(
        body: String,
    ) -> Result<(String, Result<(), String>), BoxError> {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.md");
        let out = dir.path().join("out.md");
        std::fs::write(&input, valid_implement_task_spec()).unwrap();
        let args: Vec<String> = vec![
            "--serve".into(),
            "--url".into(),
            server.uri(),
            "--context".into(),
            "C".into(),
            "wf".into(),
            "--input".into(),
            input.to_string_lossy().to_string(),
            "--out".into(),
            out.to_string_lossy().to_string(),
        ];
        let result = super::run_workflow_cmd(&args)
            .await
            .map_err(|err| err.to_string());
        let output = std::fs::read_to_string(out).unwrap_or_default();
        Ok((output, result))
    }

    #[tokio::test]
    async fn run_workflow_serve_completed_writes_artifact_and_exits_zero() {
        let body = format!(
            "{}{}",
            stream_response_sse(artifact_update("done")),
            stream_response_sse(terminal_update(a2a::TaskState::Completed))
        );
        let (output, result) = run_workflow_with_fake_serve(body).await.unwrap();
        assert!(result.is_ok(), "unexpected error: {result:?}");
        assert_eq!(output, "done");
    }

    #[tokio::test]
    async fn run_workflow_serve_failed_terminal_is_nonzero() {
        let body = stream_response_sse(terminal_update(a2a::TaskState::Failed));
        let (_output, result) = run_workflow_with_fake_serve(body).await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_workflow_serve_canceled_terminal_is_nonzero() {
        let body = stream_response_sse(terminal_update(a2a::TaskState::Canceled));
        let (_output, result) = run_workflow_with_fake_serve(body).await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_workflow_serve_jsonrpc_error_is_nonzero_without_sse_parse() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "error": {
                            "code": -32000,
                            "message": "HandleBusy"
                        }
                    })),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.md");
        std::fs::write(&input, valid_implement_task_spec()).unwrap();
        let args: Vec<String> = vec![
            "--serve".into(),
            "--url".into(),
            server.uri(),
            "--context".into(),
            "C".into(),
            "wf".into(),
            "--input".into(),
            input.to_string_lossy().to_string(),
        ];
        let err = super::run_workflow_cmd(&args)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("HandleBusy"), "unexpected error: {err}");
    }

    // ---- T10: `models` subcommand arg-parsing ----

    #[test]
    fn parse_models_args_flags() {
        let args: Vec<String> = ["--agent", "codex", "--json"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let parsed = super::parse_models_args(&args).unwrap();
        assert_eq!(parsed.agent.as_deref(), Some("codex"));
        assert!(parsed.json && parsed.config.is_none());
    }

    #[test]
    fn parse_models_args_config_and_defaults() {
        let args: Vec<String> = ["--config", "x.toml"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let parsed = super::parse_models_args(&args).unwrap();
        assert_eq!(parsed.config.as_deref(), Some("x.toml"));
        assert!(!parsed.json && parsed.agent.is_none());
    }

    #[test]
    fn parse_models_args_rejects_unknown_flag() {
        let args = vec!["--bogus".to_string()];
        assert!(super::parse_models_args(&args).is_err());
    }

    #[test]
    fn implement_input_arg_parse() {
        let a: Vec<String> = [
            "--input",
            "task.md",
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
                input,
                repo,
                base_ref,
                workflow,
            } => {
                assert_eq!(input, "task.md");
                assert_eq!(repo, std::path::PathBuf::from("/src/repo"));
                assert_eq!(base_ref.as_deref(), Some("main"));
                assert_eq!(workflow, "implement-edit");
            }
            ImplementMode::Resume { .. } => panic!("expected Fresh"),
        }
        assert_eq!(p.config, std::path::PathBuf::from("c.toml"));
        assert_eq!(p.depth, None);

        let stdin = super::parse_implement_args(&[
            "--input".into(),
            "-".into(),
            "--repo".into(),
            "/src/repo".into(),
        ])
        .unwrap();
        match stdin.mode {
            ImplementMode::Fresh { input, .. } => assert_eq!(input, "-"),
            ImplementMode::Resume { .. } => panic!("expected Fresh"),
        }

        assert!(super::parse_implement_args(&[
            "legacy positional task".into(),
            "--repo".into(),
            "/src/repo".into(),
        ])
        .is_err());
    }

    #[test]
    fn parse_implement_args_requires_input_and_repo() {
        // --repo alone is missing --input.
        assert!(super::parse_implement_args(&["--repo".into(), "/r".into()]).is_err());
        // --input present but no --repo.
        assert!(super::parse_implement_args(&["--input".into(), "task.md".into()]).is_err());
    }

    #[test]
    fn parse_implement_fresh_and_resume() {
        let fresh = super::parse_implement_args(&[
            "--input".into(),
            "task.md".into(),
            "--repo".into(),
            "/r".into(),
            "--config".into(),
            "/c.toml".into(),
        ])
        .unwrap();
        match fresh.mode {
            ImplementMode::Fresh { input, repo, .. } => {
                assert_eq!(input, "task.md");
                assert_eq!(repo, std::path::PathBuf::from("/r"));
            }
            ImplementMode::Resume { .. } => panic!("expected Fresh"),
        }
        assert_eq!(fresh.config, std::path::PathBuf::from("/c.toml"));
        assert_eq!(fresh.depth, None);

        let res = super::parse_implement_args(&["--resume".into(), "impl-1-ab".into()]).unwrap();
        match res.mode {
            ImplementMode::Resume { resume_id } => assert_eq!(resume_id, "impl-1-ab"),
            ImplementMode::Fresh { .. } => panic!("expected Resume"),
        }
        assert_eq!(res.depth, None);
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
        assert!(super::parse_implement_args(&["--input".into(), "task.md".into()]).is_err());
    }

    #[test]
    fn implement_input_writes_body_sans_frontmatter() {
        let raw = valid_implement_task_spec();
        let spec = bridge_core::task_spec::validate_input(raw).unwrap();
        let root = temp_task_spec_path("implement-body");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        super::write_implement_task_file(&root, &spec).unwrap();

        let written = std::fs::read_to_string(root.join(".git").join("A2A_TASK.md")).unwrap();
        assert_eq!(written, bridge_core::task_spec::body(&spec));
        assert!(written.starts_with("# Add task-spec CLI"));
        assert!(!written.contains("task-type: implement"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn review_sizing_diff_args_force_rename_detection() {
        let args = super::review_sizing_diff_args("base", "head");
        assert!(args.iter().any(|a| a == "--find-renames"));
        assert!(args.iter().any(|a| a == "core.quotePath=false"));
        assert_eq!(args.last().map(String::as_str), Some("base..head"));
    }

    #[test]
    fn parse_implement_depth_flag() {
        let a = review::Depth::parse_flag("light").unwrap();
        assert_eq!(a, review::Depth::Forced(review::Tier::Light));
        assert_eq!(
            review::Depth::parse_flag("standard").unwrap(),
            review::Depth::Forced(review::Tier::Standard)
        );
        assert_eq!(
            review::Depth::parse_flag("thorough").unwrap(),
            review::Depth::Forced(review::Tier::Thorough)
        );
        assert_eq!(
            review::Depth::parse_flag("auto").unwrap(),
            review::Depth::Auto
        );
        assert!(review::Depth::parse_flag("bogus").is_err());
    }

    #[test]
    fn parse_implement_args_threads_depth_thorough() {
        // Integration: --depth thorough flows through the arg parser to Some(Forced(Thorough)).
        let a: Vec<String> = ["--input", "task.md", "--repo", "/r", "--depth", "thorough"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let p = super::parse_implement_args(&a).unwrap();
        assert_eq!(p.depth, Some(review::Depth::Forced(review::Tier::Thorough)));
        // an unknown --depth value is rejected.
        let bad: Vec<String> = ["--input", "task.md", "--repo", "/r", "--depth", "bogus"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(super::parse_implement_args(&bad).is_err());
    }

    #[test]
    fn parse_implement_args_lang_flag() {
        let a = |extra: &[&str]| -> Vec<String> {
            let mut v = vec![
                "--input".to_string(),
                "task.md".to_string(),
                "--repo".to_string(),
                "/r".to_string(),
            ];
            v.extend(extra.iter().map(|s| s.to_string()));
            v
        };
        let p = super::parse_implement_args(&a(&["--lang", "go"])).unwrap();
        assert_eq!(p.lang, LangArg::Explicit("go".into()));
        let p = super::parse_implement_args(&a(&["--lang", "none"])).unwrap();
        assert_eq!(p.lang, LangArg::None);
        let p = super::parse_implement_args(&a(&[])).unwrap();
        assert_eq!(p.lang, LangArg::Auto);
        // --resume must NOT accept --lang (leaves it erroring on unknown flags)
        assert!(super::parse_implement_args(&[
            "--resume".into(),
            "x".into(),
            "--lang".into(),
            "go".into(),
        ])
        .is_err());
    }

    #[test]
    fn select_profile_explicit_go_finds_profile() {
        let toml = r#"
default = "a"
[server]
addr = "127.0.0.1:8080"
[[agents]]
id = "a"
cmd = "codex-acp"
[[languages]]
id = "go"
fetch = "go mod download"
warm_cache = "a2a-impl-lsp-cache-go"
dep_cache_path = "/gopath"
verify_cache_path = "/cache"
[[languages.verify]]
name = "build"
cmd = "go build ./..."
"#;
        let cfg = config::RegistryConfig::parse(toml).expect("parses");
        let p = super::select_profile(
            &cfg,
            &LangArg::Explicit("go".into()),
            std::path::Path::new("/"),
        )
        .expect("ok");
        assert!(p.is_some());
        assert_eq!(p.unwrap().id, "go");
    }

    #[test]
    fn select_profile_explicit_bogus_errors() {
        let toml = r#"
default = "a"
[server]
addr = "127.0.0.1:8080"
[[agents]]
id = "a"
cmd = "codex-acp"
[[languages]]
id = "rust"
fetch = "cargo fetch --locked"
warm_cache = "a2a-impl-lsp-cache"
dep_cache_path = "/cargo"
verify_cache_path = "/cache"
[[languages.verify]]
name = "build"
cmd = "cargo build --locked"
"#;
        let cfg = config::RegistryConfig::parse(toml).expect("parses");
        let result = super::select_profile(
            &cfg,
            &LangArg::Explicit("bogus".into()),
            std::path::Path::new("/"),
        );
        assert!(result.is_err(), "bogus id must error");
    }

    #[test]
    fn select_profile_none_returns_option_none() {
        let toml = r#"
default = "a"
[server]
addr = "127.0.0.1:8080"
[[agents]]
id = "a"
cmd = "codex-acp"
[[languages]]
id = "rust"
fetch = "cargo fetch --locked"
warm_cache = "a2a-impl-lsp-cache"
dep_cache_path = "/cargo"
verify_cache_path = "/cache"
[[languages.verify]]
name = "build"
cmd = "cargo build --locked"
"#;
        let cfg = config::RegistryConfig::parse(toml).expect("parses");
        let result =
            super::select_profile(&cfg, &LangArg::None, std::path::Path::new("/")).expect("ok");
        assert!(result.is_none());
    }

    #[test]
    fn select_profile_auto_detects_rust_from_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let toml = r#"
default = "a"
[server]
addr = "127.0.0.1:8080"
[[agents]]
id = "a"
cmd = "codex-acp"
[[languages]]
id = "rust"
fetch = "cargo fetch --locked"
warm_cache = "a2a-impl-lsp-cache"
dep_cache_path = "/cargo"
verify_cache_path = "/cache"
[[languages.verify]]
name = "build"
cmd = "cargo build --locked"
"#;
        let cfg = config::RegistryConfig::parse(toml).expect("parses");
        let result = super::select_profile(&cfg, &LangArg::Auto, dir.path()).expect("ok");
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "rust");
    }

    #[test]
    fn select_profile_auto_empty_dir_errors() {
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
default = "a"
[server]
addr = "127.0.0.1:8080"
[[agents]]
id = "a"
cmd = "codex-acp"
[[languages]]
id = "rust"
fetch = "cargo fetch --locked"
warm_cache = "a2a-impl-lsp-cache"
dep_cache_path = "/cargo"
verify_cache_path = "/cache"
[[languages.verify]]
name = "build"
cmd = "cargo build --locked"
"#;
        let cfg = config::RegistryConfig::parse(toml).expect("parses");
        let result = super::select_profile(&cfg, &LangArg::Auto, dir.path());
        assert!(
            result.is_err(),
            "empty dir (no language markers) must error"
        );
    }

    #[test]
    fn resume_depth_precedence() {
        use review::{Depth, Tier};

        assert_eq!(
            super::resolve_resume_depth(None, Some("thorough")),
            Depth::Forced(Tier::Thorough)
        );
        assert_eq!(super::resolve_resume_depth(None, None), Depth::Auto);
        assert_eq!(
            super::resolve_resume_depth(Some(Depth::Auto), Some("thorough")),
            Depth::Auto
        );
        assert_eq!(
            super::resolve_resume_depth(Some(Depth::Forced(Tier::Light)), Some("thorough")),
            Depth::Forced(Tier::Light)
        );
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
            None,
            120_000,
            None,
        );
        bridge_registry::registry::Registry::new(snap, spawn)
            .expect("containerized config (incl. the impl container_rw agent) validates");
    }

    #[test]
    fn depth_from_forced_str_maps_all_cases() {
        assert_eq!(
            review::Depth::from_forced_str(Some("light")),
            review::Depth::Forced(review::Tier::Light)
        );
        assert_eq!(
            review::Depth::from_forced_str(Some("standard")),
            review::Depth::Forced(review::Tier::Standard)
        );
        assert_eq!(
            review::Depth::from_forced_str(Some("thorough")),
            review::Depth::Forced(review::Tier::Thorough)
        );
        assert_eq!(review::Depth::from_forced_str(None), review::Depth::Auto);
        assert_eq!(
            review::Depth::from_forced_str(Some("bogus")),
            review::Depth::Auto
        );
    }

    #[test]
    fn reviewer_prompts_carry_line_by_line_and_git_archaeology() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts");
        let reviewers = [
            "review-implement.md",
            "review-implement-refine.md",
            "review-correctness.md",
            "review-architecture.md",
            "spec-review-rigor.md",
            "spec-review-rigor-refine.md",
            "spec-review-soundness.md",
            "spec-review-soundness-refine.md",
            "plan-review-exec.md",
            "plan-review-exec-refine.md",
            "plan-review-coverage.md",
            "plan-review-coverage-refine.md",
        ];
        for f in reviewers {
            let t = std::fs::read_to_string(dir.join(f)).unwrap();
            assert!(
                t.to_lowercase().contains("line-by-line"),
                "{f}: missing line-by-line clause"
            );
            assert!(
                t.contains("git blame") && t.contains("log -L"),
                "{f}: missing git archaeology"
            );
        }
        let ri = std::fs::read_to_string(dir.join("review-implement.md")).unwrap();
        assert!(
            ri.contains("prism") && ri.contains("nav_"),
            "review-implement missing prism block"
        );
        let synth = std::fs::read_to_string(dir.join("implement-review-light-synth.md")).unwrap();
        assert!(synth.contains("{{reviewer}}") && !synth.contains("{{reviewer_claude}}"));
        let refine = std::fs::read_to_string(dir.join("review-implement-refine.md")).unwrap();
        assert!(
            !refine.contains("VERDICT"),
            "review-implement-refine must not emit a VERDICT (synth decides)"
        );
    }

    #[test]
    fn resume_lang_arg_maps_all_cases() {
        assert_eq!(super::resume_lang_arg(&None), LangArg::Auto);
        assert_eq!(super::resume_lang_arg(&Some("none".into())), LangArg::None);
        assert_eq!(
            super::resume_lang_arg(&Some("rust".into())),
            LangArg::Explicit("rust".into())
        );
    }

    fn lsp_spec(env: Vec<(String, String)>) -> bridge_core::mcp::McpServerSpec {
        bridge_core::mcp::McpServerSpec {
            name: "lsp".into(),
            command: "/usr/local/bin/lsp-mcp".into(),
            args: vec![],
            env,
        }
    }

    fn prism_spec() -> bridge_core::mcp::McpServerSpec {
        bridge_core::mcp::McpServerSpec {
            name: "prism".into(),
            command: "/opt/prism".into(),
            args: vec![],
            env: vec![("RUST_LOG".into(), "warn".into())],
        }
    }

    #[test]
    fn apply_lsp_env_sets_env_and_overrides_same_key() {
        let mut specs = vec![
            lsp_spec(vec![
                ("LSP_MCP_LOG".into(), "/log".into()),
                ("CARGO_HOME".into(), "/old-cargo".into()),
            ]),
            prism_spec(),
        ];
        let profile_env = vec![
            ("CARGO_HOME".into(), "/cargo".into()),
            ("CARGO_NET_OFFLINE".into(), "true".into()),
        ];
        super::apply_lsp_env(&mut specs, &profile_env);
        let lsp = specs.iter().find(|s| s.name == "lsp").unwrap();
        assert!(lsp
            .env
            .iter()
            .any(|(k, v)| k == "LSP_MCP_LOG" && v == "/log"));
        assert!(
            lsp.env
                .iter()
                .any(|(k, v)| k == "CARGO_HOME" && v == "/cargo"),
            "profile must override existing CARGO_HOME"
        );
        assert!(
            lsp.env
                .iter()
                .any(|(k, v)| k == "CARGO_NET_OFFLINE" && v == "true"),
            "profile must add CARGO_NET_OFFLINE"
        );
        // EXACT order: profile env first (CARGO_*), then the config's non-overridden entry (LSP_MCP_LOG),
        // and the overridden CARGO_HOME appears ONCE with the profile value -- byte-for-byte with the
        // pre-move render order (CARGO_* before LSP_MCP_LOG).
        assert_eq!(
            lsp.env,
            vec![
                ("CARGO_HOME".into(), "/cargo".into()),
                ("CARGO_NET_OFFLINE".into(), "true".into()),
                ("LSP_MCP_LOG".into(), "/log".into()),
            ]
        );
        // prism untouched
        let prism = specs.iter().find(|s| s.name == "prism").unwrap();
        assert_eq!(prism.env, vec![("RUST_LOG".into(), "warn".into())]);
    }

    #[test]
    fn apply_lsp_env_no_op_when_no_lsp_spec() {
        let mut specs = vec![prism_spec()];
        super::apply_lsp_env(&mut specs, &[("CARGO_HOME".into(), "/cargo".into())]);
        assert_eq!(specs[0].env, vec![("RUST_LOG".into(), "warn".into())]);
    }

    #[test]
    fn drop_lsp_removes_only_lsp_spec() {
        let mut specs = vec![lsp_spec(vec![]), prism_spec()];
        super::drop_lsp(&mut specs);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "prism");
    }

    #[test]
    fn apply_warm_lsp_injects_env_mounts_and_target_vol() {
        // NOTE: the hardcoded `rust_profile()` has EMPTY lsp_env (profile.rs:119) — the real lsp_env lives on
        // the CONFIG profile (containerized.toml). So build a profile WITH lsp_env populated via `from_parts`.
        // `McpServerSpec.env` and `LanguageProfile.lsp_env` are BOTH `Vec<(String, String)>` (mcp.rs:18,
        // profile.rs:50). `lsp_env` is private → must use the constructor.
        let p = bridge_core::profile::LanguageProfile::from_parts(
            "rust".into(),
            "cargo fetch --locked".into(),
            "a2a-impl-lsp-cache".into(),
            "/cargo".into(),       // dep_cache_path → the Lsp-ctx mount target
            "/cache/cargo".into(), // verify_cache_path (unused here)
            vec![],                // fetch_env
            vec![
                // lsp_env (the thing under test)
                ("CARGO_HOME".into(), "/cargo".into()),
                ("CARGO_NET_OFFLINE".into(), "true".into()),
            ],
            vec![], // verify_env
            None,   // image
            vec![], // verify_commands
        );
        let mut mcp = vec![bridge_core::mcp::McpServerSpec {
            name: "lsp".into(),
            command: "/usr/local/bin/lsp-mcp".into(),
            args: vec!["--repo".into(), "{cwd}".into()],
            env: vec![],
        }];
        let mut vols: Vec<String> = vec![];
        super::apply_warm_lsp(
            &mut mcp,
            &mut vols,
            Some(&p),
            Some("warmvol"),
            std::path::Path::new("/tmp/repo"),
        );
        // env injected onto the lsp spec (tuple access — env is Vec<(String,String)>)
        assert!(
            mcp[0].env.iter().any(|(k, _)| k == "CARGO_HOME"),
            "lsp env missing CARGO_HOME: {:?}",
            mcp[0].env
        );
        // warm dep cache mounted (because warm vol is Some) + the per-repo target vol mounted
        assert!(
            vols.iter().any(|v| v.contains("warmvol")),
            "missing /cargo mount: {vols:?}"
        );
        assert!(
            vols.iter().any(|v| v.ends_with(":/lsp-target")),
            "missing /lsp-target mount: {vols:?}"
        );
    }
}
