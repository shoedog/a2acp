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
//   a2a-bridge validate --config <path>                  — validate config, workflows, and prompt refs

mod catalog_probe;
mod config;
mod containers;
mod doctor;
mod route;
mod slice;

pub(crate) use bridge_controller::{
    implement, implement_resume, merge, resilient, review, turn, tweak, verify,
};

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bridge_a2a_inbound::server::InboundServer;
use bridge_a2a_outbound::{
    A2aClient, ClientError, PeerDelegation, SendOpts, StreamingReply, StubDelegation, TaskIdMode,
};
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
const DEFAULT_ARTIFACT_ALLOWLIST_PATH: &str = ".github/workflow-artifact-allowlist.txt";
const DISPOSABLE_ARTIFACT_DEST: &str = "/tmp (or /private/tmp on macOS)";

fn effort_to_string(effort: &bridge_core::domain::Effort) -> String {
    match effort {
        bridge_core::domain::Effort::Minimal => "minimal".to_string(),
        bridge_core::domain::Effort::Low => "low".to_string(),
        bridge_core::domain::Effort::Medium => "medium".to_string(),
        bridge_core::domain::Effort::High => "high".to_string(),
        bridge_core::domain::Effort::Xhigh => "xhigh".to_string(),
        bridge_core::domain::Effort::Max => "max".to_string(),
    }
}

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
  validate            Validate config schema, registry, workflow DAGs, and prompt refs.
                      [--config <f>] [--examples-policy off|warn|deny] [--project-marker <text>]...
                      or --repo-hygiene [--artifact-allowlist <path>]
  serve               Run the A2A server.  [--config <path>]
  mcp                 Serve the MCP protocol over stdio (one stable Coordinator; A2A/CLI/MCP are thin adapters).
                      [--config <path>] [--store <path>]
  task-spec           Inspect or validate typed task-spec inputs. schema | template | input
  prompt              Inspect the named prompt registry ([[prompts]]). list | show <id>  [--config <f>]
  containers          List / reap this config's managed containers (crash-orphan cleanup).  list | reap
  submit              Send a unary message.  [skill] --input <file> [--context <id>] [--agent <id>] [--model <m>] [--effort <e>] [--mode <m>] [--cwd <dir>]
  task                Durable task store.  get | list | cancel | watch
  session             Warm session control.  status | release | cancel | clear | compact <contextId>
  doctor              Read-only preflight: config, agent commands/runtimes, egress, verify/review, store, MCP.  [--config <f>] [--json]

Run `a2a-bridge <subcommand> --help` for details. Quickstart + cwd/creds/concurrency notes: AGENTS.md.";

const MCP_USAGE: &str = "\
usage: a2a-bridge mcp [--config <path>] [--store <path>]
                      [--examples-policy off|warn|deny] [--project-marker <text>]...

Serve the MCP protocol over stdio. STDOUT is reserved for NDJSON MCP replies; tracing is written to STDERR.

  --config <path>  registry config (default: ./a2a-bridge.toml)
  --store <path>   override the [store] path for this MCP process
  --examples-policy off|warn|deny
                   optional examples/ hygiene policy for project-specific workflow material
  --project-marker <text>
                   non-empty marker string to match when --examples-policy is warn|deny; repeatable.
                   Passing a marker without --examples-policy implies warn.";

/// `serve` has no dedicated `_cmd` function (its body is inlined in `main()`), so this usage
/// constant lives beside the other top-level infra constants rather than beside a `fn`.
const SERVE_USAGE: &str = "\
usage: a2a-bridge serve [--config <path>]
       a2a-bridge                    (bare invocation is equivalent to `serve` with no --config)

Run the A2A server. `--config <path>` is a promise and must already exist; omitted, this reads
./a2a-bridge.toml from the CWD, which must ALSO already exist — `a2a-bridge init` is the only
thing that scaffolds a config (neither form of `serve` writes one anymore).
  --config <path>  registry config defining the agent registry + [server]/[store]/[workflows]/etc.
                   (default: ./a2a-bridge.toml)";

/// `doctor` has no dedicated `_cmd` in main.rs (it lives in `doctor.rs`), but per the W3-A convention
/// (see `SERVE_USAGE`) its usage constant is defined here so `dispatcher_help` can hand it out uniformly.
const DOCTOR_USAGE: &str = "\
usage: a2a-bridge doctor [--config <path>] [--json]

Read-only, advisory preflight: parses + validates the config, then reports on the things that most
commonly break a first run — agent commands/runtimes, api_key_env, sandbox egress (network/image),
[verify] and [review] infrastructure, the [store] path, MCP servers, the lsp_env containerized-MCP
trap, and configured credential bind-mounts. Every external probe is bounded (a wedged runtime is
reported, never hung on) and NOTHING is written to disk — doctor never spawns an agent turn, creates a
container, or touches the network beyond a local `<runtime> network|image inspect`.

  --config <path>  registry config to check (default: ./a2a-bridge.toml)
  --json           emit a stable {check, status, detail, remedy} JSON array instead of the text table

Exit code is 0 unless at least one check is `fail` (warnings alone exit 0).";

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
    Validate,
    Mcp,
    TaskSpec,
    Prompt,
    Doctor,
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
        Some("validate") => TopSubcommand::Validate,
        Some("mcp") => TopSubcommand::Mcp,
        Some("task-spec") => TopSubcommand::TaskSpec,
        Some("prompt") => TopSubcommand::Prompt,
        Some("doctor") => TopSubcommand::Doctor,
        Some("help") | Some("--help") | Some("-h") => TopSubcommand::Help,
        Some("serve") | None => TopSubcommand::Serve,
        Some(other) => TopSubcommand::Unknown(other.to_string()),
    }
}

/// Dispatcher-level `--help`/`-h` interception (wave 3, W3-A): if the first arg AFTER the
/// subcommand is `--help`/`-h`, return that subcommand's usage text so `main()` can print it
/// and exit 0 BEFORE any per-command parsing runs. Only `submit`/`task`/`session`/`serve`/
/// `merge`/`init` need this — every other subcommand already checks `--help` inside its own
/// parser. `init` is the dangerous case: its permissive `flag()` helper ignores unknown flags,
/// so without this check `a2a-bridge init --help` would silently scaffold files instead of
/// printing help. Nested forms (e.g. `task get --help`) are OUT of scope this wave — only the
/// first post-subcommand arg is checked.
///
/// `doctor` (W3-B) is ALSO listed here even though `doctor_cmd`'s own parser checks `--help` too
/// (belt-and-suspenders, matching the uniform "help is intercepted before any per-command work"
/// contract this map exists for) — unlike `mcp`, which relies solely on its own internal check.
fn dispatcher_help(sub: &TopSubcommand, raw_args: &[String]) -> Option<&'static str> {
    match raw_args.get(2).map(|s| s.as_str()) {
        Some("--help") | Some("-h") => {}
        _ => return None,
    }
    match sub {
        TopSubcommand::Submit => Some(SUBMIT_USAGE),
        TopSubcommand::Task => Some(TASK_USAGE),
        TopSubcommand::Session => Some(SESSION_USAGE),
        TopSubcommand::Serve => Some(SERVE_USAGE),
        TopSubcommand::Merge => Some(MERGE_USAGE),
        TopSubcommand::Init => Some(INIT_USAGE),
        TopSubcommand::Doctor => Some(DOCTOR_USAGE),
        _ => None,
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
        pre_authenticated: entry.pre_authenticated,
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

/// B2: run a blocking closure on tokio's blocking pool so it can't park a runtime worker. Used at the
/// few call sites where genuinely-blocking work (a container-runtime CLI shell-out) runs on a LIVE
/// async runtime — NOT for one-shot CLI commands or pre-bind boot sweeps, where a parked worker has no
/// victim. The `.expect` restores normal panic-propagation (spawn_blocking turns a panic into a JoinError).
async fn run_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f)
        .await
        .expect("blocking task panicked")
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
        pre_authenticated: entry.pre_authenticated,
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

fn worktree_runtime_parts(
    cfg: &RegistryConfig,
) -> Result<Option<(String, bridge_core::SessionCwd)>, String> {
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
    Ok(Some((root, allowed_root)))
}

/// Resolve `[worktrees]` into a runtime cfg. Worktrees are opt-in, host-only, and require
/// `allowed_cwd_root` so the decorator can self-gate before any git operation. `[worktrees]`
/// changes require a serve restart because the spawn factory captures this config once;
/// hot-reload does not re-read it.
fn resolve_worktree_runtime_cfg(
    cfg: &RegistryConfig,
) -> Result<Option<WorktreeRuntimeCfg>, String> {
    let Some((root, allowed_root)) = worktree_runtime_parts(cfg)? else {
        return Ok(None);
    };
    std::fs::create_dir_all(&root).map_err(|e| format!("[worktrees] root {root:?}: {e}"))?;
    Ok(Some(WorktreeRuntimeCfg {
        enabled: true,
        root,
        allowed_root: Some(allowed_root),
    }))
}

fn batch_runtime(
    cfg: &RegistryConfig,
    observer: Arc<dyn bridge_core::ports::Observer>,
) -> Result<Option<bridge_coordinator::BatchRuntime>, config::ConfigError> {
    Ok(cfg.batch_config()?.map(|batch| {
        bridge_coordinator::BatchRuntime::new(
            batch.max_concurrent,
            batch.default_concurrency,
            observer,
        )
    }))
}

fn turn_log_observer_enabled(
    metrics_cfg: &config::MetricsConfig,
    traces_cfg: &config::TracesConfig,
) -> bool {
    traces_cfg.enabled || (metrics_cfg.enabled && metrics_cfg.turn_log)
}

/// #10 slice 7 (refinement b): the ONE Coordinator construction shape, shared by the
/// `serve` and `mcp` entry points. They differ ONLY in `allowed_cwd_root` — serve passes
/// `None` (the wire cwd-gate stays on the adapter's `Option<String>` root; wiring the real
/// root into the Coordinator is a deferred follow-up), mcp passes the parsed root — so that
/// stays a parameter. Both wire the SAME session_manager / executor / stores / registry /
/// policy / batch plus the interactive-permission registry.
#[allow(clippy::too_many_arguments)]
fn build_coordinator(
    session_manager: Arc<bridge_coordinator::session_manager::SessionManager>,
    executor: Arc<bridge_workflow::executor::WorkflowExecutor>,
    wf_map: std::collections::HashMap<
        bridge_core::ids::WorkflowId,
        Arc<bridge_workflow::graph::WorkflowGraph>,
    >,
    task_store: Arc<dyn bridge_core::task_store::TaskStore>,
    session_store: Arc<dyn bridge_core::ports::SessionStore>,
    policy: Arc<dyn PolicyEngine>,
    registry: Arc<dyn AgentRegistry>,
    clock: Arc<dyn bridge_coordinator::clock::Clock>,
    allowed_cwd_root: Option<bridge_core::session_cwd::SessionCwd>,
    batch: Option<bridge_coordinator::BatchRuntime>,
    observer: Arc<dyn bridge_core::ports::Observer>,
    resume_cap: u32,
    trace_refs_enabled: bool,
    max_task_turns: usize,
    perm_registry: Arc<PermissionRegistry>,
) -> Arc<bridge_coordinator::Coordinator> {
    Arc::new(
        bridge_coordinator::Coordinator::new(
            session_manager,
            Some(executor),
            Arc::new(wf_map),
            task_store,
            session_store,
            policy,
            registry,
            clock,
            allowed_cwd_root,
            batch,
            observer,
            resume_cap,
        )
        .with_trace_refs_config(trace_refs_enabled, max_task_turns)
        .with_permission_registry(perm_registry),
    )
}

fn validate_worktree_runtime_cfg(cfg: &RegistryConfig) -> Result<(), String> {
    let _ = worktree_runtime_parts(cfg)?;
    Ok(())
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
usage: a2a-bridge implement --input <file|-> --repo <path> [--config <path>] [--base-ref <ref>] [--workflow <id>] [--depth auto|light|standard|thorough] [--merge [--onto <branch>]]
       a2a-bridge implement --resume <id> [--config <path>] [--merge [--onto <branch>]]
  --input <file|-> task-spec markdown to implement; use '-' to read stdin (required)
  --repo <path>   the repo to implement in; cloned into a quarantine under allowed_cwd_root (required)
  --config <path> registry config defining the impl agent + [implement]/[verify]/[review] (default: ./a2a-bridge.toml)
  --base-ref      branch/SHA to start from (default: the repo HEAD)
  --workflow <id> the edit workflow (default: implement-edit)
  --depth         review depth: auto|light|standard|thorough (default: [review].default_depth, else auto)
  --lang          language profile: auto|none|<id> (default: auto; auto detects from repo markers)
  --merge         after an Approved run, land it via `merge` (sugar for `a2a-bridge merge <id>`)
  --onto <branch> merge target when --merge is set (else [merge].target_ref, else the run's base_ref)
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

/// The real runner: spawn the container, capture stdout+stderr combined, return the exit code.
fn docker_runner(program: &str, argv: &[String]) -> std::io::Result<(i32, String)> {
    let out = std::process::Command::new(program).args(argv).output()?;
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code().unwrap_or(-1), combined))
}

/// Run the B2b-2 verify once (total). `verify_cfg` was captured pre-snapshot. The verdict run itself never
/// fails (a runner error becomes a failed result); a config error reduces to `ConfigError`.
/// Returns `Skipped` immediately when `profile` is `None` (`--lang none`).
fn run_verify_step(
    verify_cfg: &Option<Result<verify::VerifyConfig, config::ConfigError>>,
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
                &docker_runner,
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
    verify_cfg: &Option<Result<verify::VerifyConfig, config::ConfigError>>,
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
    match docker_runner(&program, &argv) {
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
        ..bridge_workflow::executor::WorkflowRunContext::default()
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
    verify_cfg: &'a Option<Result<verify::VerifyConfig, config::ConfigError>>,
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
    verify_cfg: &Option<Result<verify::VerifyConfig, config::ConfigError>>,
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
    merge_cfg: Option<Result<merge::MergeConfig, config::ConfigError>>,
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

/// Dispatcher-level `--help`/`-h` for `merge` is handled in `main.rs` (this constant is
/// `pub` so the top-level dispatcher can print it) BEFORE `merge_cmd`'s own parser runs —
/// its `--onto`/`--config`/`--force`-only loop would otherwise reject `--help` as an
/// "unexpected arg".
pub const MERGE_USAGE: &str = "\
usage: a2a-bridge merge <id> [--config <path>] [--onto <branch>] [--force]

Land an Approved `implement` run's commit into its source_repo, re-authored to the operator,
via `git commit-tree` + `git push --force-with-lease` (Mode A: fast-forward onto --onto).
  <id>             the run id (the clone dir name under .a2a-implement/)
  --config <path>  registry config providing allowed_cwd_root + [merge] (default: ./a2a-bridge.toml)
  --onto <branch>  target branch to land onto (else [merge].target_ref, else the run's base_ref)
  --force          also allow landing a LoopStopped (not Approved) run";

/// `a2a-bridge merge <id> [--config <path>] [--onto <branch>] [--force]`
pub async fn merge_cmd(args: &[String]) -> Result<(), BoxError> {
    let mut id: Option<String> = None;
    let mut config_path = std::path::PathBuf::from(CONFIG_PATH);
    let mut onto: Option<String> = None;
    let mut force = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                i += 1;
                config_path = args.get(i).ok_or("merge: --config needs a path")?.into();
            }
            "--onto" => {
                i += 1;
                onto = Some(args.get(i).ok_or("merge: --onto needs a branch")?.clone());
            }
            "--force" => force = true,
            s if !s.starts_with('-') && id.is_none() => id = Some(s.to_string()),
            s => return Err(format!("merge: unexpected arg {s:?}").into()),
        }
        i += 1;
    }
    let id =
        id.ok_or("merge: missing <id> (usage: a2a-bridge merge <id> [--onto <branch>] [--force])")?;
    let config_path = std::fs::canonicalize(&config_path)
        .map_err(|e| format!("merge: config {}: {e}", config_path.display()))?;
    let raw =
        std::fs::read_to_string(&config_path).map_err(|e| format!("merge: read config: {e}"))?;
    let cfg =
        config::RegistryConfig::parse(&raw).map_err(|e| format!("merge: config parse: {e}"))?;
    let root = cfg
        .allowed_cwd_root
        .clone()
        .ok_or("merge: config needs allowed_cwd_root")?;
    let root = std::fs::canonicalize(&root)
        .map_err(|e| format!("merge: allowed_cwd_root {root:?}: {e}"))?;
    let mcfg = cfg
        .merge
        .as_ref()
        .map(|m| m.to_config())
        .transpose()
        .map_err(|e| format!("merge: {e}"))?;
    let clone = implement_resume::resolve_clone(&root, &id).map_err(|e| format!("merge: {e}"))?;

    let outcome = merge::merge_clone(mcfg.as_ref(), &clone, &root, onto.as_deref(), force);
    use std::io::Write;
    std::io::stdout().flush().ok();
    std::process::exit(outcome.code());
}

/// Execute the `run-workflow` subcommand.
/// Loads the config, resolves the workflow graph, runs the executor,
/// prints NodeStarted/NodeFinished to stderr and the terminal output to stdout
/// (or `--out <file>`).
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
    let mut metadata = serde_json::Map::new();
    metadata.insert("a2a-bridge.skill".to_string(), workflow_id.into());
    if let Some(cwd) = session_cwd {
        metadata.insert("a2a-bridge.cwd".to_string(), cwd.into());
    }

    // Client-side taskId minting is load-bearing: the server otherwise
    // synthesises the constant stub `task-1`, which collides concurrent
    // `--serve` runs in the durable store.
    let opts = SendOpts {
        context_id: Some(context.to_string()),
        task_id: TaskIdMode::Mint,
        metadata: Some(metadata),
    };
    let parts = [bridge_core::domain::Part { text: input.into() }];

    let reply = A2aClient::loopback(url)
        .send_streaming_with(&parts, opts)
        .await
        .map_err(|e| {
            format!("cannot reach serve at {url} — is `a2a-bridge serve` running? ({e})")
        })?;

    let mut stream = match reply {
        // The server declined to stream and answered with a unary JSON body.
        StreamingReply::Json(v) => {
            if let Some(err) = v.get("error") {
                return Err(format!("run-workflow --serve failed: {err}").into());
            }
            return Err("run-workflow --serve: expected SSE response, got JSON response".into());
        }
        StreamingReply::Events(stream) => stream,
    };

    use futures::StreamExt;
    let mut output = String::new();
    let mut terminal: Option<a2a::TaskState> = None;

    while let Some(event) = stream.next().await {
        let event = event.map_err(|e| format!("run-workflow --serve: stream error: {e}"))?;
        let response: a2a::StreamResponse = serde_json::from_str(&event.data)
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
                ..bridge_workflow::executor::WorkflowRunContext::default()
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
  Pass models only when model_configurable=true; effort/mode only when those lists are present.
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
                    let configurable = if caps.models.is_empty() || caps.model_configurable {
                        ""
                    } else {
                        "; model override unavailable"
                    };
                    println!(
                        "{id}: {}  (current: {current}{configurable})",
                        caps.models.join(", ")
                    );
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

/// Generic loopback JSON-RPC call against a running `serve`.
///
/// Delegates transport to `A2aClient::loopback(url).rpc(...)` (no Authorization,
/// no total timeout) and maps `ClientError` back to the exact user-facing
/// strings. Result-shape interpretation (`v["result"]`, `v["error"]`) stays in
/// the individual callers.
async fn rpc_call(
    url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, BoxError> {
    A2aClient::loopback(url)
        .rpc(method, params)
        .await
        .map_err(|e| -> BoxError {
            match e {
                ClientError::Transport(_) => {
                    format!("cannot reach serve at {url} — is `a2a-bridge serve` running? ({e})")
                        .into()
                }
                ClientError::Decode(_) => format!("bad response: {e}").into(),
                // rpc() parses the body regardless of status, so a Status error is only a
                // non-JSON non-success body (e.g. a 500 HTML page). Surface it verbatim.
                ClientError::Status { .. } => e.to_string().into(),
            }
        })
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

const SUBMIT_USAGE: &str = "\
usage: a2a-bridge submit [skill] --input <file> [--url <url>] [--context <id>] [--agent <id>]
                         [--model <m>] [--effort <e>] [--mode <m>] [--cwd <dir>]

Send one unary message to a running a2a-bridge serve and print the response text.
  [skill]         optional skill/workflow name (the first non-flag argument)
  --input <file>  message body to send (required)
  --url <url>     serve URL (default: http://127.0.0.1:8080)
  --context <id>  reuse an existing contextId (continues that warm session)
  --agent <id>    route to a specific agent id
  --model <m>     override the agent model for this message
  --effort <e>    override the reasoning effort for this message
  --mode <m>      override the agent mode for this message
  --cwd <dir>     override the session cwd for this message";

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

const TASK_USAGE: &str = "\
usage: a2a-bridge task get <id> [--url <url>]
       a2a-bridge task list [--limit <n>] [--url <url>]
       a2a-bridge task cancel <id> [--url <url>]
       a2a-bridge task watch <id> [--from <seq>] [--url <url>]

Durable task store against a running a2a-bridge serve (default url http://127.0.0.1:8080).
  get <id>     print the task record as JSON
  list         list tasks, newest first (--limit, default 50)
  cancel <id>  request cancellation; prints the task record as JSON
  watch <id>   stream the task's progress over SSE (--from <seq> resumes after a cursor)";

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

const SESSION_USAGE: &str = "\
usage: a2a-bridge session status <contextId> [--url <url>]
       a2a-bridge session release <contextId> [--url <url>]
       a2a-bridge session cancel <contextId> [--url <url>]
       a2a-bridge session clear <contextId> [--force] [--url <url>]
       a2a-bridge session compact <contextId> [--url <url>]
       a2a-bridge session inject <contextId> --input <file> [--append] [--dedupe <key>] [--url <url>]
       a2a-bridge session permit <requestId> --context <contextId> --generation <n> --op <operationId>
                      (--approve | --deny | --modify <optionId> | --escalate)
                      [--option <id>] [--reason <text>] [--url <url>]

Warm session control against a running a2a-bridge serve (default url http://127.0.0.1:8080).
  status|release|cancel   inspect / free / abort the warm session for <contextId>
  clear                   reset to a fresh generation (--force overrides a live-turn guard)
  compact                 summarize + reset, seeding the summary as the next turn's prefix
  inject                  queue --input text to prepend (default) or append to the next turn
  permit                  resolve a pending interactive permission request";

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
    let reply = A2aClient::loopback(url)
        .subscribe_sse(
            a2a::methods::SUBSCRIBE_TO_TASK,
            serde_json::json!({ "id": id }),
            from.map(|cursor| cursor.to_string()),
        )
        .await
        .map_err(|e| {
            format!("cannot reach serve at {url} — is `a2a-bridge serve` running? ({e})")
        })?;

    let mut stream = match reply {
        StreamingReply::Events(stream) => stream,
        // The server declined to stream and answered with a unary JSON body.
        StreamingReply::Json(v) => {
            if let Some(err) = v.get("error") {
                return Err(format!("task watch failed: {err}").into());
            }
            return Err("task watch: expected SSE response, got JSON response".into());
        }
    };

    // Print each event's data payload; track the last `id:` for the resume hint.
    use futures::StreamExt;
    let mut last_id: Option<String> = None;

    while let Some(event) = stream.next().await {
        let event = event.map_err(|e| format!("task watch: stream error: {e}"))?;
        println!("{}", event.data);
        if let Some(seq) = event.id {
            last_id = Some(seq);
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

/// Resolve the config path shared by `serve` and `mcp`: an explicit `--config <path>` is a
/// PROMISE and must already exist; omitting it reads `./a2a-bridge.toml` (`CONFIG_PATH`),
/// which must ALSO already exist. Wave 3 removed the zero-config auto-write both callers used
/// to do on the implicit path — `a2a-bridge init` is now the only thing that writes a config,
/// so both branches share this one missing-config error + `init` hint.
fn require_config_path(explicit: Option<PathBuf>) -> Result<PathBuf, BoxError> {
    let p = explicit.unwrap_or_else(|| PathBuf::from(CONFIG_PATH));
    if !p.exists() {
        return Err(format!(
            "a2a-bridge: config not found at {}; run `a2a-bridge init` to create one",
            p.display()
        )
        .into());
    }
    Ok(p)
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
            "\n# kiro: zero-auth local default (kiro-cli acp). ACP SDK 1.x can discover Kiro's\n# native model list, but cannot apply Kiro model pins unless the catalog marks\n# the agent `model_configurable: true`; leave model unpinned by default.\n[[agents]]\nid   = \"kiro\"\ncmd  = \"kiro-cli\"\nargs = [\"acp\"]\n"
        }
        "codex" => {
            "\n# codex: gpt-5.5 with reasoning_effort. Run `codex login` first; the\n# existing login is ambient, so the bridge must not restart browser auth.\n[[agents]]\nid    = \"codex\"\ncmd   = \"codex-acp\"\npre_authenticated = true\nmodel = \"gpt-5.5\"\neffort = \"high\"\n"
        }
        "claude" => {
            "\n# claude: subscription. `model` is validated against the advertised values and\n# applied. Fable ids are blocked by this bridge; use another advertised model.\n[[agents]]\nid    = \"claude\"\ncmd   = \"claude-agent-acp\"\nmodel = \"sonnet\"\n"
        }
        "api" => {
            "\n# api: OpenAI-compatible non-process backend. `api_key_env` is the NAME of an\n# env var holding the token (never the secret itself). Effort is not applied for api.\n[[agents]]\nid          = \"api\"\nkind        = \"api\"\nbase_url    = \"https://api.openai.com/v1\"\napi_key_env = \"OPENAI_API_KEY\"\nmodel       = \"gpt-4o-mini\"\n"
        }
        _ => "",
    }
}

/// The review workflows and their named prompt registry entries for `init` output.
/// All reference both `codex` and `claude`, so they're only emitted when both
/// are selected (else `load_workflows` would fail on a missing agent at boot).
const INIT_WORKFLOWS: &str = r#"
[[prompts]]
id = "review-correctness"
file = "prompts/review-correctness.md"
description = "code-review correctness lens"

[[prompts]]
id = "review-architecture"
file = "prompts/review-architecture.md"
description = "code-review architecture lens"

[[prompts]]
id = "review-synth"
file = "prompts/review-synth.md"
description = "code-review synthesis prompt"

[[prompts]]
id = "review-implement"
file = "prompts/review-implement.md"
description = "shared implement-review prompt"

[[prompts]]
id = "review-implement-synth"
file = "prompts/review-implement-synth.md"
description = "implement-review synthesis prompt"

[[prompts]]
id = "spec-review-rigor"
file = "prompts/spec-review-rigor.md"
description = "spec-review rigor lens"

[[prompts]]
id = "spec-review-soundness"
file = "prompts/spec-review-soundness.md"
description = "spec-review soundness lens"

[[prompts]]
id = "spec-review-synth"
file = "prompts/spec-review-synth.md"
description = "spec-review synthesis prompt"

[[prompts]]
id = "plan-review-exec"
file = "prompts/plan-review-exec.md"
description = "plan-review executability lens"

[[prompts]]
id = "plan-review-coverage"
file = "prompts/plan-review-coverage.md"
description = "plan-review coverage lens"

[[prompts]]
id = "plan-review-synth"
file = "prompts/plan-review-synth.md"
description = "plan-review synthesis prompt"

[[prompts]]
id = "design-executability"
file = "prompts/design-executability.md"
description = "design executability lens"

[[prompts]]
id = "design-structure"
file = "prompts/design-structure.md"
description = "design structure lens"

[[prompts]]
id = "design-synth"
file = "prompts/design-synth.md"
description = "design synthesis prompt"

# ── Review workflows (two independent lenses + a synthesis) ──
[[workflows]]
id = "code-review"
[[workflows.nodes]]
id = "correctness"
agent = "codex"
prompt = "review-correctness"
inputs = []
[[workflows.nodes]]
id = "architecture"
agent = "claude"
prompt = "review-architecture"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt = "review-synth"
inputs = ["correctness", "architecture"]

# ── implement-review (B2b-3a): two folded reviewers of the committed diff → synth verdict ──
[[workflows]]
id = "implement-review"
[[workflows.nodes]]
id = "reviewer_codex"
agent = "codex"
prompt = "review-implement"
inputs = []
[[workflows.nodes]]
id = "reviewer_claude"
agent = "claude"
prompt = "review-implement"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt = "review-implement-synth"
inputs = ["reviewer_codex", "reviewer_claude"]

[[workflows]]
id = "spec-review"
[[workflows.nodes]]
id = "rigor"
agent = "codex"
prompt = "spec-review-rigor"
inputs = []
[[workflows.nodes]]
id = "soundness"
agent = "claude"
prompt = "spec-review-soundness"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt = "spec-review-synth"
inputs = ["rigor", "soundness"]

[[workflows]]
id = "plan-review"
[[workflows.nodes]]
id = "exec"
agent = "codex"
prompt = "plan-review-exec"
inputs = []
[[workflows.nodes]]
id = "coverage"
agent = "claude"
prompt = "plan-review-coverage"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt = "plan-review-synth"
inputs = ["exec", "coverage"]

# design: two clean-room architect lenses (firewalled via inputs=[]) + synth.
[[workflows]]
id = "design"
[[workflows.nodes]]
id = "executability"
agent = "codex"
prompt = "design-executability"
inputs = []
[[workflows.nodes]]
id = "structure"
agent = "claude"
prompt = "design-structure"
inputs = []
[[workflows.nodes]]
id = "synth"
agent = "claude"
prompt = "design-synth"
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

const INIT_USAGE: &str = "\
usage: a2a-bridge init [--dir <path>] [--agents kiro,codex,claude,api] [--default <id>] [--force]

Scaffold a working a2a-bridge.toml + prompts/*.md + README-a2a-bridge.md + a .a2a-bridge/ store
dir into --dir (default: .). The ONLY thing that writes an a2a-bridge.toml — `serve`/`mcp`
now hard-error on a missing config instead of auto-writing one.
  --dir <path>    destination directory (default: .)
  --agents <csv>  comma-separated agent ids to include (default: kiro,codex,claude,api)
  --default <id>  override the top-level default agent (must be among --agents)
  --force         overwrite existing managed files (refuses by default)";

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
    let mut examples_policy = None;
    let mut project_markers = Vec::new();
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
            "--examples-policy" => {
                examples_policy = Some(ExamplesPolicy::parse(
                    iter.next()
                        .ok_or("mcp: --examples-policy requires off|warn|deny")?,
                )?);
            }
            "--project-marker" => {
                project_markers.push(
                    iter.next()
                        .ok_or("mcp: --project-marker requires a value")?
                        .to_string(),
                );
            }
            other => {
                return Err(format!("mcp: unknown flag {other:?}\n{MCP_USAGE}").into());
            }
        }
    }

    let config_path = require_config_path(explicit_config)?;
    let config_path = std::fs::canonicalize(&config_path).map_err(|e| {
        format!(
            "a2a-bridge: cannot resolve config path {}: {e}",
            config_path.display()
        )
    })?;
    let examples_policy = finalize_examples_policy(examples_policy, &project_markers)?;
    let validation = validate_config_file(
        &config_path,
        examples_policy,
        &project_markers,
        ValidationScope::Startup,
    )
    .map_err(|e| format!("mcp: config validation failed: {e}"))?;
    for warning in validation.warnings {
        eprintln!("a2a-bridge mcp: warning: {warning}");
    }

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

    cfg.metrics_config()?;
    let allowed_cwd_root = cfg
        .allowed_cwd_root
        .as_deref()
        .map(bridge_core::session_cwd::SessionCwd::parse)
        .transpose()
        .map_err(|e| format!("a2a-bridge mcp: invalid allowed_cwd_root: {e:?}"))?;
    let observer: Arc<dyn bridge_core::ports::Observer> = Arc::new(bridge_observ::NoopObserver);
    let batch =
        batch_runtime(&cfg, Arc::clone(&observer)).map_err(|e| format!("a2a-bridge mcp: {e}"))?;

    let coordinator = build_coordinator(
        session_manager,
        executor,
        wf_map,
        task_store,
        session_store,
        Arc::clone(&policy) as Arc<dyn PolicyEngine>,
        Arc::clone(&registry) as Arc<dyn AgentRegistry>,
        clock,
        allowed_cwd_root,
        batch,
        observer,
        resume_cap,
        false,
        512,
        Arc::clone(&perm_registry),
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

const VALIDATE_USAGE: &str = "\
usage: a2a-bridge validate [--config <path>] [--examples-policy off|warn|deny] [--project-marker <text>]...
       a2a-bridge validate --repo-hygiene [--artifact-allowlist <path>]

Validate a bridge config without spawning agents. This parses the config, eagerly resolves
workflow prompt files and named prompts, validates workflow DAGs, builds the registry snapshot,
and runs the registry validation gate.

Examples policy: configs/prompts/workflows owned by another codebase should live in that
codebase, or in /tmp (or /private/tmp on macOS) for disposable local runs. Use
--examples-policy deny with one or more --project-marker values in CI or cleanup gates to
reject project-specific workflow material under an examples/ directory. Passing
--project-marker without --examples-policy implies warn.

Repo hygiene: --repo-hygiene rejects untracked root examples/*.toml and prompts/*.md,
rejects tracked root workflow artifacts not listed in .github/workflow-artifact-allowlist.txt,
rejects stale allowlist entries, and validates tracked root examples/*.toml configs.
Staged but uncommitted root artifacts are treated as tracked and require an intentional allowlist
update before commit.
--repo-hygiene cannot be combined with --config, --examples-policy, or --project-marker.
--artifact-allowlist is valid only with --repo-hygiene; relative paths are resolved from the
Git repository root.

Scope: validate covers config parsing, workflows, prompts, registry startup, serve/mcp startup
sections, language profile syntax, and may run a local git root check for [worktrees]. It does
not execute agent or container subprocesses. [implement], [review], [merge], and [verify]
details are checked by their owning subcommands when invoked.
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExamplesPolicy {
    Off,
    Warn,
    Deny,
}

impl ExamplesPolicy {
    fn parse(s: &str) -> Result<Self, BoxError> {
        match s {
            "off" => Ok(Self::Off),
            "warn" => Ok(Self::Warn),
            "deny" => Ok(Self::Deny),
            other => {
                Err(format!("invalid --examples-policy {other:?} (expected off|warn|deny)").into())
            }
        }
    }
}

fn finalize_examples_policy(
    mode: Option<ExamplesPolicy>,
    markers: &[String],
) -> Result<ExamplesPolicy, BoxError> {
    if markers.iter().any(|marker| marker.trim().is_empty()) {
        return Err("examples policy markers must be non-empty".into());
    }
    match mode {
        Some(ExamplesPolicy::Warn | ExamplesPolicy::Deny) if markers.is_empty() => Err(
            "examples policy requires at least one --project-marker when mode is warn or deny"
                .into(),
        ),
        Some(mode) => Ok(mode),
        None if markers.is_empty() => Ok(ExamplesPolicy::Off),
        None => Ok(ExamplesPolicy::Warn),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidationScope {
    Startup,
    Full,
}

#[derive(Debug)]
struct ConfigValidationReport {
    config_path: PathBuf,
    agent_count: usize,
    workflow_count: usize,
    prompt_count: usize,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ValidateMode {
    Config {
        config: PathBuf,
        examples_policy: ExamplesPolicy,
        project_markers: Vec<String>,
    },
    RepoHygiene {
        allowlist_path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoHygieneReport {
    allowlist_path: PathBuf,
    allowlist_rel: String,
    tracked_artifact_count: usize,
    validated_example_config_count: usize,
}

#[cfg(test)]
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn examples_ancestor(path: &Path) -> Option<PathBuf> {
    let mut cursor = path.parent();
    while let Some(dir) = cursor {
        if dir.file_name().and_then(|s| s.to_str()) == Some("examples") {
            return Some(dir.to_path_buf());
        }
        cursor = dir.parent();
    }
    None
}

fn contains_project_marker(text: &str, markers: &[String]) -> bool {
    let lower = text.to_ascii_lowercase();
    markers
        .iter()
        .any(|marker| lower.contains(&marker.to_ascii_lowercase()))
}

fn has_project_marker_in_examples(config_path: &Path, raw: &str, markers: &[String]) -> bool {
    if markers.is_empty() {
        return false;
    }
    let Ok(config) = std::fs::canonicalize(config_path) else {
        return false;
    };
    if examples_ancestor(&config).is_none() {
        return false;
    }
    contains_project_marker(raw, markers)
}

fn examples_policy_warning_for_config(config_path: &Path) -> Vec<String> {
    let Some(examples) = examples_ancestor(config_path) else {
        return Vec::new();
    };
    vec![format!(
        "{} appears to contain project-specific workflow material under examples/ directory {}; \
         keep owning-project configs, prompts, and workflows in that project's repo (for example \
         tools/a2a-bridge/) or in {DISPOSABLE_ARTIFACT_DEST} for disposable local runs",
        config_path.display(),
        examples.display()
    )]
}

fn validate_config_file(
    config_path: &Path,
    examples_policy: ExamplesPolicy,
    project_markers: &[String],
    scope: ValidationScope,
) -> Result<ConfigValidationReport, BoxError> {
    let config_path = std::fs::canonicalize(config_path)
        .map_err(|e| format!("cannot resolve config path {}: {e}", config_path.display()))?;
    let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("read {}: {e}", config_path.display()))?;

    let cfg = RegistryConfig::parse(&raw).map_err(|e| format!("config parse: {e}"))?;
    let base = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let workflows = cfg
        .load_workflows(base)
        .map_err(|e| format!("workflow load: {e}"))?;
    let named_prompts = config::resolve_prompt_registry(&cfg.prompts, base)
        .map_err(|e| format!("prompt registry: {e}"))?;
    let warnings = match examples_policy {
        ExamplesPolicy::Off => Vec::new(),
        ExamplesPolicy::Warn | ExamplesPolicy::Deny => {
            let config_in_examples = examples_ancestor(&config_path).is_some();
            let raw_match = has_project_marker_in_examples(&config_path, &raw, project_markers);
            let workflow_prompt_match = workflows.values().any(|workflow| {
                workflow
                    .nodes
                    .iter()
                    .any(|node| contains_project_marker(&node.prompt_template, project_markers))
            });
            let named_prompt_match = named_prompts
                .values()
                .any(|prompt| contains_project_marker(&prompt.template, project_markers));
            if config_in_examples && (raw_match || workflow_prompt_match || named_prompt_match) {
                examples_policy_warning_for_config(&config_path)
            } else {
                Vec::new()
            }
        }
    };
    if examples_policy == ExamplesPolicy::Deny && !warnings.is_empty() {
        return Err(format!("examples policy denied: {}", warnings.join("; ")).into());
    }
    if scope == ValidationScope::Full {
        cfg.language_profiles()
            .map_err(|e| format!("language profiles: {e}"))?;
    }
    validate_worktree_runtime_cfg(&cfg).map_err(|e| e.to_string())?;
    cfg.metrics_config().map_err(|e| e.to_string())?;
    batch_runtime(&cfg, Arc::new(bridge_observ::NoopObserver)).map_err(|e| e.to_string())?;
    let agent_count = cfg.agents.len();
    let prompt_count = cfg.prompts.len();
    let workflow_count = workflows.len();
    let snap = cfg
        .into_snapshot()
        .map_err(|e| format!("registry snapshot: {e}"))?;

    // Registry::new validates the snapshot without resolving any agent. Keep the validate path explicit:
    // if this invariant changes, the no-op spawn fails instead of starting agents or containers.
    let spawn: SpawnFn = Arc::new(|_| {
        Box::pin(async {
            Err(BridgeError::ConfigInvalid {
                reason: "validate must not spawn agents".into(),
            })
        })
    });
    bridge_registry::registry::Registry::new(snap, spawn)
        .map_err(|e| format!("registry: {}", e.client_message()))?;

    Ok(ConfigValidationReport {
        config_path,
        agent_count,
        workflow_count,
        prompt_count,
        warnings,
    })
}

fn parse_validate_args(args: &[String]) -> Result<ValidateMode, BoxError> {
    let mut config = None;
    let mut repo_hygiene = false;
    let mut allowlist_path = None;
    let mut examples_policy = None;
    let mut project_markers = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => {
                config = Some(
                    it.next()
                        .ok_or("validate: --config requires a <path>")?
                        .into(),
                );
            }
            "--repo-hygiene" => {
                repo_hygiene = true;
            }
            "--artifact-allowlist" => {
                allowlist_path = Some(
                    it.next()
                        .ok_or("validate: --artifact-allowlist requires a <path>")?
                        .into(),
                );
            }
            "--examples-policy" => {
                examples_policy = Some(ExamplesPolicy::parse(
                    it.next()
                        .ok_or("validate: --examples-policy requires off|warn|deny")?,
                )?);
            }
            "--project-marker" => {
                project_markers.push(
                    it.next()
                        .ok_or("validate: --project-marker requires a value")?
                        .to_string(),
                );
            }
            other => {
                return Err(format!("validate: unknown flag {other:?}\n{VALIDATE_USAGE}").into())
            }
        }
    }

    if repo_hygiene {
        let mut conflicts = Vec::new();
        if config.is_some() {
            conflicts.push("--config");
        }
        if examples_policy.is_some() {
            conflicts.push("--examples-policy");
        }
        if !project_markers.is_empty() {
            conflicts.push("--project-marker");
        }
        if !conflicts.is_empty() {
            return Err(format!(
                "validate: --repo-hygiene cannot be combined with {}\n{VALIDATE_USAGE}",
                conflicts.join(", ")
            )
            .into());
        }
        return Ok(ValidateMode::RepoHygiene { allowlist_path });
    }

    if allowlist_path.is_some() {
        return Err(format!(
            "validate: --artifact-allowlist requires --repo-hygiene\n{VALIDATE_USAGE}"
        )
        .into());
    }

    Ok(ValidateMode::Config {
        config: config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH)),
        examples_policy: finalize_examples_policy(examples_policy, &project_markers)?,
        project_markers,
    })
}

fn git_output(repo_root: &Path, argv: &[&str]) -> Result<String, BoxError> {
    // Deliberately mirrors implement::git_ok's success semantics while adapting the error type.
    let out = implement::run_git(Some(repo_root), argv)
        .map_err(|e| format!("git {}: {e}", argv.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            argv.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn repo_root_from_git(start_cwd: &Path) -> Result<PathBuf, BoxError> {
    let root = git_output(start_cwd, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(root))
}

fn artifact_allowlist_regeneration_command(allowlist_rel: &str) -> String {
    format!(
        "git ls-files ':(glob)examples/*.toml' ':(glob)prompts/*.md' | LC_ALL=C sort > {}",
        shell_single_quote(allowlist_rel)
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn is_root_example_config_path(path: &str) -> bool {
    if let Some(name) = path.strip_prefix("examples/") {
        !name.is_empty() && !name.contains('/') && name.ends_with(".toml")
    } else {
        false
    }
}

fn is_root_prompt_path(path: &str) -> bool {
    if let Some(name) = path.strip_prefix("prompts/") {
        !name.is_empty() && !name.contains('/') && name.ends_with(".md")
    } else {
        false
    }
}

fn is_root_workflow_artifact_path(path: &str) -> bool {
    is_root_example_config_path(path) || is_root_prompt_path(path)
}

fn parse_artifact_allowlist(raw: &str) -> Result<BTreeSet<String>, BoxError> {
    if raw
        .lines()
        .next()
        .is_some_and(|first_line| first_line.starts_with('\u{FEFF}'))
    {
        return Err("artifact allowlist must not start with a UTF-8 BOM".into());
    }

    let mut entries = BTreeSet::new();
    let mut previous = None::<String>;
    for (idx, line) in raw.lines().enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let line_no = idx + 1;
        if line.is_empty() {
            return Err(format!("artifact allowlist contains blank line at {line_no}").into());
        }
        if Path::new(line).is_absolute() {
            return Err(format!(
                "artifact allowlist path must be relative at line {line_no}: {line:?}"
            )
            .into());
        }
        if !line.is_ascii() {
            return Err(format!(
                "artifact allowlist path must be ASCII at line {line_no}: {line:?}"
            )
            .into());
        }
        // Root-only by design; nested paths belong in owning projects or a follow-up policy.
        if !is_root_workflow_artifact_path(line) {
            return Err(format!(
                "artifact allowlist path is outside root examples/*.toml or prompts/*.md at line {line_no}: {line:?}"
            )
            .into());
        }
        if !entries.insert(line.to_string()) {
            return Err(format!("artifact allowlist contains duplicate entry {line:?}").into());
        }
        if previous.as_ref().is_some_and(|prev| line < prev.as_str()) {
            return Err(format!(
                "artifact allowlist is not sorted at line {line_no}: {line:?} sorts before {:?}",
                previous.as_deref().unwrap_or("")
            )
            .into());
        }
        previous = Some(line.to_string());
    }
    Ok(entries)
}

fn read_artifact_allowlist(path: &Path, allowlist_rel: &str) -> Result<BTreeSet<String>, BoxError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!(
                "artifact allowlist not found at {}; regenerate it from the repository root with:\n  {}",
                path.display(),
                artifact_allowlist_regeneration_command(allowlist_rel)
            )
        } else {
            format!("read artifact allowlist {}: {e}", path.display())
        }
    })?;
    parse_artifact_allowlist(&raw)
}

fn repo_relative_path_string(repo_root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(repo_root).ok()?;
    if rel.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return None;
    }
    Some(rel.to_string_lossy().replace('\\', "/"))
}

fn require_repo_relative_path_string(repo_root: &Path, path: &Path) -> Result<String, BoxError> {
    repo_relative_path_string(repo_root, path).ok_or_else(|| {
        format!(
            "repo hygiene: artifact allowlist {} must live under repository root {}",
            path.display(),
            repo_root.display()
        )
        .into()
    })
}

fn git_lines_set(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn tracked_root_artifacts(repo_root: &Path) -> Result<BTreeSet<String>, BoxError> {
    let output = git_output(
        repo_root,
        &[
            "ls-files",
            "--",
            ":(glob)examples/*.toml",
            ":(glob)prompts/*.md",
        ],
    )?;
    Ok(git_lines_set(&output))
}

fn untracked_root_artifacts(repo_root: &Path) -> Result<Vec<String>, BoxError> {
    let output = git_output(
        repo_root,
        &[
            "ls-files",
            "--others",
            "--",
            ":(glob)examples/*.toml",
            ":(glob)prompts/*.md",
        ],
    )?;
    let mut artifacts: Vec<_> = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    artifacts.sort();
    Ok(artifacts)
}

fn ensure_allowlist_tracked(
    repo_root: &Path,
    allowlist_path: &Path,
    allowlist_rel: &str,
) -> Result<(), BoxError> {
    if is_root_workflow_artifact_path(allowlist_rel) {
        return Err(format!(
            "repo hygiene: artifact allowlist path {allowlist_rel} is itself a workflow artifact; move the allowlist outside examples/ and prompts/"
        )
        .into());
    }
    let file_tracking_result =
        git_lines_set(&git_output(repo_root, &["ls-files", "--", allowlist_rel])?);
    if file_tracking_result.contains(allowlist_rel) {
        return Ok(());
    }
    if !allowlist_path.exists() {
        return Err(format!(
            "artifact allowlist not found at {}; regenerate it from the repository root with:\n  {}",
            allowlist_path.display(),
            artifact_allowlist_regeneration_command(allowlist_rel)
        )
        .into());
    }
    Err(format!(
        "repo hygiene: artifact allowlist {allowlist_rel} is not tracked by git; add it before relying on validate --repo-hygiene in CI"
    )
    .into())
}

fn ensure_allowlist_within_repo(repo_root: &Path, allowlist_path: &Path) -> Result<(), BoxError> {
    let canonical_allowlist = match allowlist_path.canonicalize() {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(format!(
                "repo hygiene: resolve artifact allowlist {}: {e}",
                allowlist_path.display()
            )
            .into());
        }
    };
    let canonical_repo = repo_root.canonicalize().map_err(|e| {
        format!(
            "repo hygiene: resolve repository root {}: {e}",
            repo_root.display()
        )
    })?;
    if !canonical_allowlist.starts_with(&canonical_repo) {
        return Err(format!(
            "repo hygiene: artifact allowlist {} resolves outside repository root {}",
            allowlist_path.display(),
            repo_root.display()
        )
        .into());
    }
    Ok(())
}

fn unstaged_hygiene_paths(repo_root: &Path, allowlist_rel: &str) -> Result<Vec<String>, BoxError> {
    let output = git_output(
        repo_root,
        &[
            "diff",
            "--name-only",
            "--",
            ":(glob)examples/*.toml",
            ":(glob)prompts/*.md",
            allowlist_rel,
        ],
    )?;
    let mut paths: Vec<_> = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    paths.sort();
    Ok(paths)
}

fn resolve_artifact_allowlist_path(repo_root: &Path, allowlist_path: Option<&Path>) -> PathBuf {
    let path = allowlist_path.unwrap_or_else(|| Path::new(DEFAULT_ARTIFACT_ALLOWLIST_PATH));
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    }
}

fn ensure_artifact_allowlist_matches(
    tracked: &BTreeSet<String>,
    allowlist: &BTreeSet<String>,
    allowlist_rel: &str,
) -> Result<(), BoxError> {
    let missing: Vec<_> = tracked.difference(allowlist).cloned().collect();
    let stale: Vec<_> = allowlist.difference(tracked).cloned().collect();
    if missing.is_empty() && stale.is_empty() {
        return Ok(());
    }

    let mut msg =
        String::from("repo hygiene: tracked root workflow artifacts and allowlist differ");
    if !missing.is_empty() {
        msg.push_str("\ntracked artifacts missing from allowlist:");
        for path in &missing {
            msg.push_str(&format!("\n  {path}"));
        }
    }
    if !stale.is_empty() {
        msg.push_str("\nallowlist entries without tracked files:");
        for path in &stale {
            msg.push_str(&format!("\n  {path}"));
        }
    }
    msg.push_str("\nmove project-owned files to their owning repo, or intentionally update the artifact allowlist from the repository root with:");
    msg.push_str(&format!(
        "\n  {}",
        artifact_allowlist_regeneration_command(allowlist_rel)
    ));
    Err(msg.into())
}

fn validate_repo_hygiene_at(
    repo_root: &Path,
    allowlist_path: Option<&Path>,
) -> Result<RepoHygieneReport, BoxError> {
    let allowlist_path = resolve_artifact_allowlist_path(repo_root, allowlist_path);
    let allowlist_rel = require_repo_relative_path_string(repo_root, &allowlist_path)?;

    let untracked = untracked_root_artifacts(repo_root)?;
    if !untracked.is_empty() {
        let mut msg = format!(
            "repo hygiene: untracked root workflow artifacts found; move generated or project-specific files to the owning project repo or {DISPOSABLE_ARTIFACT_DEST}:"
        );
        for path in &untracked {
            msg.push_str(&format!("\n  {path}"));
        }
        return Err(msg.into());
    }

    let unstaged = unstaged_hygiene_paths(repo_root, &allowlist_rel)?;
    if !unstaged.is_empty() {
        let mut msg = String::from(
            "repo hygiene: unstaged changes found in guarded workflow artifact paths; stage or revert them before running validate --repo-hygiene:",
        );
        for path in &unstaged {
            msg.push_str(&format!("\n  {path}"));
        }
        return Err(msg.into());
    }

    let tracked = tracked_root_artifacts(repo_root)?;
    ensure_allowlist_tracked(repo_root, &allowlist_path, &allowlist_rel)?;
    ensure_allowlist_within_repo(repo_root, &allowlist_path)?;
    let allowlist = read_artifact_allowlist(&allowlist_path, &allowlist_rel)?;
    // ASCII-only artifact pathnames are an intentional repo invariant; Rust str ordering matches
    // `LC_ALL=C sort` for the current allowlist under that invariant.
    ensure_artifact_allowlist_matches(&tracked, &allowlist, &allowlist_rel)?;

    let mut validated_example_config_count = 0;
    let mut config_errors = Vec::new();
    // If a root prompts/ file is deleted or renamed, tracked examples/*.toml that reference it must
    // be updated or removed too. This validation intentionally catches stale prompt references.
    for rel in tracked
        .iter()
        .filter(|path| is_root_example_config_path(path))
    {
        validated_example_config_count += 1;
        let path = repo_root.join(rel);
        if let Err(e) = validate_config_file(&path, ExamplesPolicy::Off, &[], ValidationScope::Full)
        {
            config_errors.push(format!("{rel}: {e}"));
        }
    }
    if !config_errors.is_empty() {
        let mut msg = String::from("repo hygiene: tracked example configs failed validation");
        for error in &config_errors {
            msg.push_str(&format!("\n  {error}"));
        }
        return Err(msg.into());
    }

    Ok(RepoHygieneReport {
        allowlist_path,
        allowlist_rel,
        tracked_artifact_count: tracked.len(),
        validated_example_config_count,
    })
}

fn format_repo_hygiene_report(report: &RepoHygieneReport) -> String {
    format!(
        "repository hygiene validated\nallowlist: {}\ntracked_artifacts: {}\nvalidated_example_configs: {}\n",
        report.allowlist_rel,
        report.tracked_artifact_count,
        report.validated_example_config_count
    )
}

fn validate_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{VALIDATE_USAGE}");
        return Ok(());
    }

    match parse_validate_args(args)? {
        ValidateMode::Config {
            config,
            examples_policy,
            project_markers,
        } => {
            let report = validate_config_file(
                &config,
                examples_policy,
                &project_markers,
                ValidationScope::Full,
            )
            .map_err(|e| {
                let msg = e.to_string();
                if msg.starts_with("cannot resolve config path") {
                    format!(
                        "a2a-bridge: config not found or inaccessible at {}; run `a2a-bridge init` to create one",
                        config.display()
                    )
                } else {
                    format!("validate: {msg}")
                }
            })?;
            println!("validated {}", report.config_path.display());
            println!("agents: {}", report.agent_count);
            println!("workflows: {}", report.workflow_count);
            println!("named_prompts: {}", report.prompt_count);
            for warning in &report.warnings {
                eprintln!("warning: {warning}");
            }
            Ok(())
        }
        ValidateMode::RepoHygiene { allowlist_path } => {
            let cwd = std::env::current_dir().map_err(|e| format!("validate: current dir: {e}"))?;
            let repo_root = repo_root_from_git(&cwd).map_err(|e| format!("validate: {e}"))?;
            let report = validate_repo_hygiene_at(&repo_root, allowlist_path.as_deref())
                .map_err(|e| format!("validate: {e}"))?;
            print!("{}", format_repo_hygiene_report(&report));
            Ok(())
        }
    }
}

const PROMPT_USAGE: &str = "\
usage: a2a-bridge prompt list [--config <path>]
       a2a-bridge prompt show <id> [--config <path>]

Inspect the named prompt registry ([[prompts]]). `list` shows ids + descriptions;
`show <id>` prints the raw template. Default config is ./a2a-bridge.toml.";

/// Core for `prompt list`: read prompts (NO file I/O on `file=`), validate ids + reject dups, sort by id.
fn prompt_list_lines(config_path: &std::path::Path) -> Result<Vec<String>, BoxError> {
    use bridge_core::ids::PromptId;
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("prompt: read {config_path:?}: {e}"))?;
    let prompts = config::parse_prompts_only(&raw).map_err(|e| format!("{e}"))?;
    let mut by_id: std::collections::BTreeMap<PromptId, Option<String>> =
        std::collections::BTreeMap::new();
    for p in &prompts {
        let id = PromptId::parse(p.id.clone())
            .map_err(|_| format!("prompt id {:?} is invalid", p.id))?;
        if by_id.insert(id, p.description.clone()).is_some() {
            return Err(format!("duplicate prompt id {:?}", p.id).into());
        }
    }
    Ok(by_id
        .into_iter()
        .map(|(id, desc)| {
            format!(
                "{} — {}",
                id.as_str(),
                desc.as_deref().unwrap_or("(no description)")
            )
        })
        .collect())
}

/// Core for `prompt show <id>`: validate ids + dedup (no read), then resolve ONLY the requested entry.
fn prompt_show_text(config_path: &std::path::Path, id: &str) -> Result<String, BoxError> {
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("prompt: read {config_path:?}: {e}"))?;
    let prompts = config::parse_prompts_only(&raw).map_err(|e| format!("{e}"))?;
    let mut seen = std::collections::HashSet::new();
    for p in &prompts {
        bridge_core::ids::PromptId::parse(p.id.clone())
            .map_err(|_| format!("prompt id {:?} is invalid", p.id))?;
        if !seen.insert(p.id.as_str()) {
            return Err(format!("duplicate prompt id {:?}", p.id).into());
        }
    }
    let base = config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    match prompts.iter().find(|p| p.id == id) {
        Some(entry) => Ok(config::resolve_one(entry, base)
            .map_err(|e| format!("{e}"))?
            .template),
        None => {
            let mut ids: Vec<&str> = prompts.iter().map(|p| p.id.as_str()).collect();
            ids.sort_unstable();
            Err(format!("unknown prompt {id:?}; available: [{}]", ids.join(", ")).into())
        }
    }
}

fn prompt_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{PROMPT_USAGE}");
        return Ok(());
    }
    if args.iter().any(|a| a == "--resolved") {
        return Err(
            format!("prompt: --resolved is reserved for a later release\n{PROMPT_USAGE}").into(),
        );
    }
    let mut config = std::path::PathBuf::from(CONFIG_PATH);
    let mut positional: Vec<&String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => {
                config = it.next().ok_or("prompt: --config requires a value")?.into();
            }
            s if s.starts_with("--") => {
                return Err(format!("prompt: unknown flag {a:?}\n{PROMPT_USAGE}").into())
            }
            _ => positional.push(a),
        }
    }
    match positional.first().map(|s| s.as_str()) {
        Some("list") => {
            if positional.len() > 1 {
                return Err(format!(
                    "prompt list: unexpected argument {:?}\n{PROMPT_USAGE}",
                    positional[1]
                )
                .into());
            }
            for line in prompt_list_lines(&config)? {
                println!("{line}");
            }
            Ok(())
        }
        Some("show") => {
            let id = positional
                .get(1)
                .ok_or_else(|| format!("prompt show: expected <id>\n{PROMPT_USAGE}"))?;
            if positional.len() > 2 {
                return Err(format!(
                    "prompt show: unexpected argument {:?}\n{PROMPT_USAGE}",
                    positional[2]
                )
                .into());
            }
            print!("{}", prompt_show_text(&config, id)?);
            Ok(())
        }
        Some(other) => Err(format!("prompt: unknown subcommand {other:?}\n{PROMPT_USAGE}").into()),
        None => Err(format!("prompt: missing subcommand\n{PROMPT_USAGE}").into()),
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
    let sub = parse_top_subcommand(&raw_args);
    // Dispatcher-level `--help`/`-h` for the subcommands whose own parser doesn't (yet) check
    // it: see `dispatcher_help`'s doc comment for why `init` in particular needs this BEFORE
    // any per-command parsing runs.
    if let Some(usage) = dispatcher_help(&sub, &raw_args) {
        println!("{usage}");
        return Ok(());
    }
    match sub {
        TopSubcommand::RunWorkflow => return run_workflow_cmd(&raw_args[2..]).await,
        TopSubcommand::RunBatch => return run_batch_cmd(&raw_args[2..]).await,
        TopSubcommand::Batch => return batch_cmd(&raw_args[2..]).await,
        TopSubcommand::Models => return models_cmd(&raw_args[2..]).await,
        TopSubcommand::Implement => return implement_cmd(&raw_args[2..]).await,
        TopSubcommand::Merge => return merge_cmd(&raw_args[2..]).await,
        TopSubcommand::Containers => return containers_cmd(&raw_args[2..]),
        TopSubcommand::Submit => return submit_cmd(&raw_args[2..]).await,
        TopSubcommand::Task => return task_cmd(&raw_args[2..]).await,
        TopSubcommand::Session => return session_cmd(&raw_args[2..]).await,
        TopSubcommand::Init => return init_cmd(&raw_args[2..]),
        TopSubcommand::Validate => return validate_cmd(&raw_args[2..]),
        TopSubcommand::Mcp => return mcp_cmd(&raw_args[2..]).await,
        TopSubcommand::TaskSpec => return task_spec_cmd(&raw_args[2..]),
        TopSubcommand::Prompt => return prompt_cmd(&raw_args[2..]),
        TopSubcommand::Doctor => return doctor::doctor_cmd(&raw_args[2..]),
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
                "a2a-bridge: unknown subcommand {other:?} (expected: serve | mcp | run-workflow | run-batch | batch | models | implement | merge | containers | submit | task | task-spec | prompt | session | init | validate | doctor | help)"
            )
            .into());
        }
    }

    // 1. Observability — install tracing subscriber (idempotent).
    bridge_observ::init();

    // 2. Configuration. `serve --config <path>` reads an EXPLICIT config (must already
    //    exist — an explicit path is a promise, so a missing one errors with an `init`
    //    hint). Bare `a2a-bridge` (or `serve` with no --config) reads ./a2a-bridge.toml,
    //    which must ALSO already exist (wave 3: neither form silently scaffolds a config
    //    anymore — `a2a-bridge init` is the only writer). The path is absolutised so
    //    workflow prompt + relative store paths resolve against the config's OWN
    //    directory, not the process CWD.
    let explicit_config = if raw_args.get(1).map(|s| s.as_str()) == Some("serve") {
        serve_config_flag(&raw_args[2..])?
    } else {
        None
    };
    let config_path = require_config_path(explicit_config)?;
    let config_path = std::fs::canonicalize(&config_path).map_err(|e| {
        format!(
            "a2a-bridge: cannot resolve config path {}: {e}",
            config_path.display()
        )
    })?;
    let validation = validate_config_file(
        &config_path,
        ExamplesPolicy::Off,
        &[],
        ValidationScope::Startup,
    )
    .map_err(|e| format!("serve: config validation failed: {e}"))?;
    for warning in validation.warnings {
        eprintln!("a2a-bridge serve: warning: {warning}");
    }

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
                // B2: recover_orphans shells out to the container runtime (blocking, potentially seconds
                // against a wedged daemon) and runs on the LIVE serve runtime here — offload to the
                // blocking pool so it can't park a tokio worker. Awaited INLINE to preserve the
                // recover-before-apply ordering.
                let snap_c = snap.clone();
                let config_c = recover_config.clone();
                let host_c = recover_host.clone();
                run_blocking(move || recover_orphans(&snap_c, &config_c, &host_c)).await;
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

    let metrics_cfg = cfg.metrics_config()?;
    let traces_cfg = cfg.traces_config()?;
    let prometheus_observer = if metrics_cfg.enabled && metrics_cfg.prometheus {
        let vocab = bridge_observ::LabelVocabulary {
            agents: probe_entries
                .iter()
                .map(|(agent, _)| agent.as_str().to_string())
                .collect(),
            models: probe_entries
                .iter()
                .filter_map(|(_, entry)| entry.model.clone())
                .collect(),
            efforts: probe_entries
                .iter()
                .filter_map(|(_, entry)| entry.effort.as_ref().map(effort_to_string))
                .collect(),
        };
        Some(Arc::new(bridge_observ::PrometheusObserver::new(vocab)?))
    } else {
        None
    };

    if let Some(prom) = &prometheus_observer {
        let rows = task_store.turn_log_rows().await.unwrap_or_else(|e| {
            tracing::warn!(error = ?e, "turn_log rebuild skipped");
            Vec::new()
        });
        prom.rebuild_from_turn_log(&rows);
    }

    let dedupe = if metrics_cfg.enabled {
        prometheus_observer
            .as_ref()
            .map(|p| p.dedupe())
            .unwrap_or_else(|| Arc::new(bridge_observ::TurnDedupe::default()))
    } else {
        Arc::new(bridge_observ::TurnDedupe::default())
    };

    let install_turn_log = turn_log_observer_enabled(&metrics_cfg, &traces_cfg);
    let observer: Arc<dyn bridge_core::ports::Observer> =
        if !metrics_cfg.enabled && !install_turn_log {
            Arc::new(bridge_observ::NoopObserver)
        } else {
            let mut sinks: Vec<Arc<dyn bridge_core::ports::Observer>> = Vec::new();
            if let Some(prom) = &prometheus_observer {
                sinks.push(prom.clone());
            }
            if install_turn_log {
                let dropped = prometheus_observer
                    .as_ref()
                    .map(|p| p.drop_counter())
                    .unwrap_or_else(bridge_observ::DropCounter::disabled);
                sinks.push(Arc::new(bridge_observ::TurnLogObserver::new(
                    task_store.clone(),
                    dropped,
                    1024,
                    Arc::new(|| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64
                    }),
                )));
            }

            Arc::new(bridge_observ::DedupObserver::new_with_dedupe(
                Arc::new(bridge_observ::FanoutObserver::new(sinks)),
                dedupe,
            ))
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
    // #10 slice 1: the SessionManager and the Coordinator (built below) share ONE clock.
    // SystemClock is behaviourally identical to `SessionManager::new`'s internal default,
    // but making it explicit lets both owners of turn-lifecycle state read one time source.
    let clock: Arc<dyn bridge_coordinator::clock::Clock> =
        Arc::new(bridge_coordinator::clock::SystemClock);
    let warm_ttl = cfg.server.warm_idle_ttl_secs;
    let registry_for_sessions: Arc<dyn AgentRegistry> = registry.clone();
    let session_manager = Arc::new(
        bridge_a2a_inbound::session_manager::SessionManager::new_with_clock(
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

    // 8. Build the ONE Coordinator (#10 slice 1), then construct the inbound server
    //    ADOPTING its shared turn-lifecycle STATE. The Coordinator owns the state;
    //    A2A becomes a co-equal adapter over the SAME instances (no parallel copies).
    //    D1: the ONE in-memory `store` is instance-shared as the Coordinator's
    //    session_store (NOT a second store, NOT file-backed). B3: ONE BatchRuntime.
    let base_url = format!("http://{}", cfg.server.addr);
    let session_store: Arc<dyn bridge_core::ports::SessionStore> = store;
    let batch = batch_runtime(&cfg, Arc::clone(&observer))?;
    // #10 slice 1: the Coordinator's `allowed_cwd_root` is INERT until a handler
    // delegates a cwd-gated op (batch/detached submit — a later slice). Passing
    // `None` now keeps boot behavior-preserving: the A2A wire cwd-gate still runs
    // off the adapter's `Option<String>` root (`with_allowed_cwd_root` below), and
    // parsing the config string here would add a new boot-time failure mode
    // (`SessionCwd::parse` rejects empty/relative roots) that serve never had. The
    // real parsed root is wired at the slice that consumes it.
    let coordinator = build_coordinator(
        session_manager,
        executor,
        wf_map,
        task_store,
        Arc::clone(&session_store),
        Arc::clone(&policy),
        Arc::clone(&registry) as Arc<dyn AgentRegistry>,
        clock,
        None,
        batch,
        Arc::clone(&observer),
        resume_cap,
        traces_cfg.enabled,
        traces_cfg.max_task_turns,
        Arc::clone(&perm_registry),
    );

    // The inbound server is a thin adapter over the ONE Coordinator (#10 slice 7):
    // the Coordinator owns the registry/policy/stores/session-manager/workflow maps/
    // batch; the adapter keeps only wire state (route/auth/base_url/delegation/label/
    // model_catalog + the Option<String> cwd-gate root). Boot resume runs via
    // `coordinator.resume()` below (slice 4) — exactly ONE resume path, over the SAME
    // shared store the detached submits write to.
    let server = Arc::new(
        InboundServer::from_coordinator(
            Arc::clone(&coordinator),
            route,
            auth,
            base_url,
            delegation,
            default_label.clone(),
        )
        .with_allowed_cwd_root(cfg.allowed_cwd_root.clone())
        .with_model_catalog(Arc::clone(&model_catalog))
        .with_trace_http_config(bridge_a2a_inbound::server::TraceHttpConfig {
            enabled: traces_cfg.enabled,
            journal_max_bytes: traces_cfg.journal_max_bytes,
            journal_max_events: traces_cfg.journal_max_events,
            artifact_max_bytes: traces_cfg.artifact_max_bytes,
            max_task_turns: traces_cfg.max_task_turns,
        })
        .with_metrics_endpoint(prometheus_observer.as_ref().map(|p| p.endpoint())),
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
    //     #10 slice 4: resume via the Coordinator (its resume() dispatches to the SAME
    //     batch::resume_all / detached::resume_non_batch_tasks over the SHARED store +
    //     BatchRuntime as the adapter's resume_working_tasks). This REPLACES the adapter
    //     call — NEVER both, or a working task double-spawns two runners (Fable M4).
    coordinator.resume().await;

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

    #[test]
    fn turn_log_observer_enabled_for_traces_even_without_metrics() {
        let metrics = config::MetricsConfig {
            enabled: false,
            prometheus: false,
            turn_log: false,
        };
        let traces = config::TracesConfig {
            enabled: true,
            journal_max_bytes: 16,
            journal_max_events: 16,
            artifact_max_bytes: 16,
            max_task_turns: 2,
        };

        assert!(turn_log_observer_enabled(&metrics, &traces));
    }

    #[test]
    fn turn_log_observer_enabled_for_metrics_turn_log_without_traces() {
        let metrics = config::MetricsConfig {
            enabled: true,
            prometheus: true,
            turn_log: true,
        };
        let traces = config::TracesConfig {
            enabled: false,
            journal_max_bytes: 16,
            journal_max_events: 16,
            artifact_max_bytes: 16,
            max_task_turns: 2,
        };

        assert!(turn_log_observer_enabled(&metrics, &traces));
    }

    #[test]
    fn turn_log_observer_disabled_when_neither_surface_needs_rows() {
        let metrics = config::MetricsConfig {
            enabled: true,
            prometheus: true,
            turn_log: false,
        };
        let traces = config::TracesConfig {
            enabled: false,
            journal_max_bytes: 16,
            journal_max_events: 16,
            artifact_max_bytes: 16,
            max_task_turns: 2,
        };

        assert!(!turn_log_observer_enabled(&metrics, &traces));
    }

    /// B2 (T-B1): `run_blocking` must run its closure OFF the runtime worker. On a current-thread
    /// runtime (the `#[tokio::test]` default), a ticker on the single worker can only advance while
    /// the blocking closure sleeps IF that closure is on the blocking pool; if it ran inline it would
    /// stall the worker and the ticker couldn't tick. Direct proof the offload removes the block.
    #[tokio::test]
    async fn run_blocking_offloads_off_the_runtime_worker() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let ticks = Arc::new(AtomicUsize::new(0));
        let ticks_c = Arc::clone(&ticks);
        let ticker = tokio::spawn(async move {
            for _ in 0..40 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                ticks_c.fetch_add(1, Ordering::SeqCst);
            }
        });
        // Block a thread for ~100ms; the current-thread worker is free to run the ticker only if this
        // is on the blocking pool.
        run_blocking(|| std::thread::sleep(std::time::Duration::from_millis(100))).await;
        let during = ticks.load(Ordering::SeqCst);
        assert!(
            during >= 3,
            "ticker must advance while run_blocking's closure sleeps on the blocking pool; got {during}"
        );
        ticker.abort();
    }

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
            pre_authenticated: false,
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
    fn merge_usage_matches_the_actual_parser() {
        // `merge_cmd`'s loop accepts exactly --config/--onto/--force plus a positional <id>;
        // keep the usage constant honest against that, not a guess (W3-A).
        assert!(MERGE_USAGE.starts_with("usage: a2a-bridge merge <id>"));
        for flag in ["--config <path>", "--onto <branch>", "--force"] {
            assert!(
                MERGE_USAGE.contains(flag),
                "missing {flag:?}: {MERGE_USAGE}"
            );
        }
    }

    // ---- W3-A: dispatcher-level `--help`/`-h` + silent-config-write removal ----

    #[test]
    fn dispatcher_help_covers_the_six_newly_helped_subcommands() {
        // Before wave 3 these six had NO `--help` handling at all (they'd fall straight into
        // their own arg parser, which either errors on the unknown `--help` token or — for
        // `init` — silently ignores it and scaffolds files). The dispatcher must now intercept
        // `--help`/`-h` as the FIRST post-subcommand arg for every one of them.
        let cases: &[(&str, &str)] = &[
            ("submit", SUBMIT_USAGE),
            ("task", TASK_USAGE),
            ("session", SESSION_USAGE),
            ("serve", SERVE_USAGE),
            ("merge", MERGE_USAGE),
            ("init", INIT_USAGE),
        ];
        for (word, expected) in cases {
            assert!(
                expected.starts_with(&format!("usage: a2a-bridge {word}")),
                "{word} usage constant must open with its own usage header: {expected:?}"
            );
            for help in ["--help", "-h"] {
                let args = vec!["a2a-bridge".to_string(), word.to_string(), help.to_string()];
                let sub = parse_top_subcommand(&args);
                assert_eq!(
                    dispatcher_help(&sub, &args),
                    Some(*expected),
                    "{word} {help} should print its usage and exit 0"
                );
            }
        }
    }

    #[test]
    fn dispatcher_help_covers_doctor() {
        // W3-B: `doctor --help`/`-h` must be intercepted before `doctor_cmd`'s own parsing runs —
        // exactly the same contract as the six W3-A subcommands above (see that test + the
        // `dispatcher_help` doc comment for why `doctor` is listed even though its own parser
        // also checks `--help` defensively).
        assert!(DOCTOR_USAGE.starts_with("usage: a2a-bridge doctor"));
        for help in ["--help", "-h"] {
            let args = vec![
                "a2a-bridge".to_string(),
                "doctor".to_string(),
                help.to_string(),
            ];
            let sub = parse_top_subcommand(&args);
            assert_eq!(
                dispatcher_help(&sub, &args),
                Some(DOCTOR_USAGE),
                "doctor {help} should print its usage and exit 0"
            );
        }
    }

    #[test]
    fn dispatcher_help_does_not_fire_outside_its_narrow_contract() {
        // Not the first post-subcommand arg (nested forms are out of scope this wave).
        let nested = vec![
            "a2a-bridge".to_string(),
            "task".to_string(),
            "get".to_string(),
            "--help".to_string(),
        ];
        assert_eq!(
            dispatcher_help(&parse_top_subcommand(&nested), &nested),
            None
        );
        // A normal (non-help) invocation of a newly-helped subcommand must pass through
        // untouched, so real usage still reaches the subcommand's own handler.
        let normal = vec![
            "a2a-bridge".to_string(),
            "init".to_string(),
            "--agents".to_string(),
            "kiro".to_string(),
        ];
        assert_eq!(
            dispatcher_help(&parse_top_subcommand(&normal), &normal),
            None
        );
        // A subcommand that already handles its own `--help` (e.g. `mcp`) is NOT in the
        // dispatcher's map — it must stay None so `mcp_cmd`'s own check (still) prints
        // MCP_USAGE.
        let mcp = vec![
            "a2a-bridge".to_string(),
            "mcp".to_string(),
            "--help".to_string(),
        ];
        assert_eq!(dispatcher_help(&parse_top_subcommand(&mcp), &mcp), None);
        // Bare invocation (no subcommand at all) has nothing at index 2 to check.
        let bare = vec!["a2a-bridge".to_string()];
        assert_eq!(dispatcher_help(&parse_top_subcommand(&bare), &bare), None);
    }

    #[test]
    fn dispatcher_help_intercepts_init_before_any_scaffold() {
        // The dangerous case (verified in the spec): `init`'s permissive `flag()` helper
        // ignores unknown flags, so `init --help` used to fall through into `init_cmd` and
        // scaffold a full config + prompts. Mirror main()'s real prelude — parse the
        // subcommand, then check `dispatcher_help` BEFORE ever calling `init_cmd` — and prove
        // the `--dir` target (present in argv, exactly as a real invocation would have it) is
        // never touched.
        let dir = tempfile::tempdir().unwrap();
        let args = vec![
            "a2a-bridge".to_string(),
            "init".to_string(),
            "--help".to_string(),
            "--dir".to_string(),
            dir.path().to_string_lossy().to_string(),
        ];
        let sub = parse_top_subcommand(&args);
        let usage = dispatcher_help(&sub, &args)
            .expect("dispatcher must intercept `init --help` before init_cmd's parser runs");
        assert!(usage.starts_with("usage: a2a-bridge init"));
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "init --help must not scaffold any files"
        );
    }

    #[test]
    fn require_config_path_none_matches_the_bare_serve_and_mcp_default() {
        // No explicit --config: bare `a2a-bridge`/`serve` and `mcp` with no --config both
        // resolve the same relative CONFIG_PATH ("a2a-bridge.toml") against the process cwd.
        // Cargo runs this crate's unit tests with cwd == the package dir (bin/a2a-bridge/),
        // which has no such file, so this exercises the real "no --config, no file present"
        // path without mutating the process cwd (unsafe to do under parallel test threads).
        let err = require_config_path(None).unwrap_err().to_string();
        assert!(err.contains("a2a-bridge.toml"), "{err}");
        assert!(err.contains("run `a2a-bridge init`"), "{err}");
    }

    #[test]
    fn require_config_path_missing_explicit_errors_with_init_hint_and_writes_nothing() {
        // Represents `serve --config <path>` / `mcp --config <path>` pointed at a config that
        // doesn't exist yet. Wave 3 removed the silent DEFAULT_CONFIG auto-write both callers
        // used to do here — it must now hard-error with the `init` hint and touch NOTHING.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("a2a-bridge.toml");
        let err = require_config_path(Some(missing.clone()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("run `a2a-bridge init`"), "{err}");
        assert!(
            !missing.exists(),
            "must never write the config it was asked for"
        );
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "must create no files at all in the config's directory"
        );

        // An existing config still resolves Ok, unchanged (init remains the only writer).
        std::fs::write(&missing, "default = \"kiro\"\n").unwrap();
        assert_eq!(require_config_path(Some(missing.clone())).unwrap(), missing);
    }

    #[tokio::test]
    async fn mcp_cmd_missing_explicit_config_errors_with_init_hint_and_creates_no_file() {
        // End-to-end through the real `mcp_cmd` entry point (not just the shared helper):
        // proves the wiring, not only the helper in isolation.
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("a2a-bridge.toml");
        let err = mcp_cmd(&["--config".to_string(), cfg.to_string_lossy().to_string()])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("run `a2a-bridge init`"), "{err}");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "mcp must not scaffold a2a-bridge.toml on a missing config (init is the only writer)"
        );
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
        assert!(cfg.contains("pre_authenticated = true"));
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
    fn init_scaffold_resolves_named_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_str().unwrap();
        init_cmd(&[
            "--dir".into(),
            d.into(),
            "--agents".into(),
            "codex,claude".into(),
        ])
        .unwrap();
        let cfg = dir.path().join("a2a-bridge.toml");
        let lines = prompt_list_lines(&cfg).unwrap();
        assert!(
            !lines.is_empty(),
            "init should scaffold at least one named prompt"
        );
        let raw = std::fs::read_to_string(&cfg).unwrap();
        assert!(raw.contains("[[prompts]]") && raw.contains("prompt = \""));
        let workflows = toml::from_str::<config::RegistryConfig>(&raw)
            .unwrap()
            .load_workflows(dir.path())
            .unwrap();
        let workflow_id = bridge_core::ids::WorkflowId::parse("code-review").unwrap();
        let node_id = bridge_core::ids::NodeId::parse("correctness").unwrap();
        let node = workflows
            .get(&workflow_id)
            .unwrap()
            .nodes
            .iter()
            .find(|candidate| candidate.id == node_id)
            .unwrap();
        assert_eq!(
            node.prompt_template,
            include_str!("../../../prompts/review-correctness.md")
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
    fn validate_subcommand_is_registered_and_validates_reference_configs() {
        let raw_args: Vec<String> = ["a2a-bridge", "validate", "--config", "x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            super::parse_top_subcommand(&raw_args),
            super::TopSubcommand::Validate
        );

        let root = super::repo_root();
        for rel in [
            "examples/a2a-bridge.multi-agent.toml",
            "examples/a2a-bridge.containerized.toml",
            "examples/a2a-bridge.containerized.podman.toml",
            "examples/a2a-bridge.workflows.toml",
            "examples/a2a-bridge.panel.toml",
        ] {
            let report = super::validate_config_file(
                &root.join(rel),
                super::ExamplesPolicy::Off,
                &[],
                super::ValidationScope::Full,
            )
            .unwrap();
            assert!(
                report.workflow_count > 0,
                "{rel} should load at least one workflow"
            );
        }
    }

    fn validate_repo_hygiene_args(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    fn validate_repo_hygiene_git(repo: &std::path::Path, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .status()
                .unwrap()
                .success(),
            "git {args:?} should succeed"
        );
    }

    fn validate_repo_hygiene_temp_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().to_path_buf();
        validate_repo_hygiene_git(&repo, &["init", "-q", "-b", "main"]);
        validate_repo_hygiene_git(&repo, &["config", "user.name", "t"]);
        validate_repo_hygiene_git(&repo, &["config", "user.email", "t@t"]);
        std::fs::write(repo.join("README.md"), "hi\n").unwrap();
        validate_repo_hygiene_git(&repo, &["add", "README.md"]);
        validate_repo_hygiene_git(&repo, &["commit", "-q", "-m", "init"]);
        (td, repo)
    }

    fn validate_repo_hygiene_write(repo: &std::path::Path, rel: &str, contents: &str) {
        let path = repo.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn validate_repo_hygiene_write_allowlist(repo: &std::path::Path, entries: &[&str]) {
        let mut raw = entries.join("\n");
        if !raw.is_empty() {
            raw.push('\n');
        }
        validate_repo_hygiene_write(repo, DEFAULT_ARTIFACT_ALLOWLIST_PATH, &raw);
        validate_repo_hygiene_stage(repo, &[DEFAULT_ARTIFACT_ALLOWLIST_PATH]);
    }

    fn validate_repo_hygiene_stage(repo: &std::path::Path, rels: &[&str]) {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(repo).arg("add").args(rels);
        assert!(cmd.status().unwrap().success(), "git add {rels:?}");
    }

    fn validate_repo_hygiene_valid_config() -> &'static str {
        r#"
default = "codex"

[[agents]]
id = "codex"
cmd = "codex-acp"

[server]

[[workflows]]
id = "review"

[[workflows.nodes]]
id = "n"
agent = "codex"
prompt_file = "../prompts/project.md"
"#
    }

    fn validate_repo_hygiene_write_valid_artifacts(repo: &std::path::Path) {
        validate_repo_hygiene_write(
            repo,
            "examples/good.toml",
            validate_repo_hygiene_valid_config(),
        );
        validate_repo_hygiene_write(repo, "prompts/project.md", "review this\n");
        validate_repo_hygiene_stage(repo, &["examples/good.toml", "prompts/project.md"]);
        validate_repo_hygiene_write_allowlist(repo, &["examples/good.toml", "prompts/project.md"]);
    }

    #[test]
    fn validate_repo_hygiene_parse_validate_args_modes() {
        match super::parse_validate_args(&[]).unwrap() {
            super::ValidateMode::Config {
                config,
                examples_policy,
                project_markers,
            } => {
                assert_eq!(config, std::path::PathBuf::from(super::CONFIG_PATH));
                assert_eq!(examples_policy, super::ExamplesPolicy::Off);
                assert!(project_markers.is_empty());
            }
            other => panic!("expected config mode, got {other:?}"),
        }

        match super::parse_validate_args(&validate_repo_hygiene_args(&[
            "--repo-hygiene",
            "--artifact-allowlist",
            ".github/workflow-artifact-allowlist.txt",
        ]))
        .unwrap()
        {
            super::ValidateMode::RepoHygiene {
                allowlist_path: Some(path),
            } => assert_eq!(
                path,
                std::path::PathBuf::from(".github/workflow-artifact-allowlist.txt")
            ),
            other => panic!("expected repo hygiene mode, got {other:?}"),
        }

        for args in [
            &["--repo-hygiene", "--config", "x"][..],
            &["--repo-hygiene", "--examples-policy", "warn"][..],
            &["--repo-hygiene", "--project-marker", "x"][..],
            &[
                "--repo-hygiene",
                "--examples-policy",
                "warn",
                "--project-marker",
                "x",
            ][..],
            &["--artifact-allowlist", "x"][..],
        ] {
            let err = super::parse_validate_args(&validate_repo_hygiene_args(args))
                .unwrap_err()
                .to_string();
            assert!(err.contains(super::VALIDATE_USAGE));
        }

        match super::parse_validate_args(&validate_repo_hygiene_args(&[
            "--project-marker",
            "prism-mcp",
        ]))
        .unwrap()
        {
            super::ValidateMode::Config {
                examples_policy,
                project_markers,
                ..
            } => {
                assert_eq!(examples_policy, super::ExamplesPolicy::Warn);
                assert_eq!(project_markers, vec!["prism-mcp".to_string()]);
            }
            other => panic!("expected config mode, got {other:?}"),
        }

        assert!(super::VALIDATE_USAGE.contains("--repo-hygiene"));
        assert!(super::VALIDATE_USAGE.contains("/tmp (or /private/tmp on macOS)"));
    }

    #[test]
    fn validate_repo_hygiene_allowlist_parser_rejects_bad_content() {
        let parsed =
            super::parse_artifact_allowlist("examples/a.toml\r\nprompts/a.md\r\n").unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(parsed.contains("examples/a.toml"));
        assert!(parsed.contains("prompts/a.md"));

        for (raw, expected) in [
            ("\n", "blank line"),
            ("examples/a.toml\nexamples/a.toml\n", "duplicate"),
            ("prompts/a.md\nexamples/a.toml\n", "not sorted"),
            ("/tmp/a.toml\n", "relative"),
            ("examples/nested/foo.toml\n", "outside root"),
            ("prompts/nested/foo.md\n", "outside root"),
            ("examples/foo.md\n", "outside root"),
            ("other/foo.toml\n", "outside root"),
            ("examples/résumé.toml\n", "ASCII"),
            ("\u{FEFF}examples/a.toml\n", "UTF-8 BOM"),
        ] {
            let err = super::parse_artifact_allowlist(raw)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains(expected),
                "expected {expected:?} in error {err:?}"
            );
        }
    }

    #[test]
    fn validate_repo_hygiene_allowlist_comparison_and_report_format() {
        let tracked: BTreeSet<String> = ["examples/a.toml", "prompts/a.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let allowlist = tracked.clone();
        super::ensure_artifact_allowlist_matches(
            &tracked,
            &allowlist,
            DEFAULT_ARTIFACT_ALLOWLIST_PATH,
        )
        .unwrap();

        let stale: BTreeSet<String> = ["examples/missing.toml", "prompts/a.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let err =
            super::ensure_artifact_allowlist_matches(&tracked, &stale, "custom/allowlist.txt")
                .unwrap_err()
                .to_string();
        assert!(err.contains("examples/a.toml"));
        assert!(err.contains("examples/missing.toml"));
        assert!(err.contains("> 'custom/allowlist.txt'"));
        assert!(
            super::artifact_allowlist_regeneration_command("custom/my allowlist.txt")
                .contains("> 'custom/my allowlist.txt'")
        );

        let report = super::RepoHygieneReport {
            allowlist_path: std::path::PathBuf::from(".github/workflow-artifact-allowlist.txt"),
            allowlist_rel: ".github/workflow-artifact-allowlist.txt".to_string(),
            tracked_artifact_count: 2,
            validated_example_config_count: 1,
        };
        let output = super::format_repo_hygiene_report(&report);
        assert!(output.contains("allowlist: .github/workflow-artifact-allowlist.txt"));
        assert!(output.contains("tracked_artifacts: 2"));
        assert!(output.contains("validated_example_configs: 1"));
    }

    #[test]
    fn validate_repo_hygiene_git_backed_checks_cover_artifacts() {
        let (_td, repo) = validate_repo_hygiene_temp_repo();
        validate_repo_hygiene_write_valid_artifacts(&repo);
        let report = super::validate_repo_hygiene_at(&repo, None).unwrap();
        assert_eq!(report.tracked_artifact_count, 2);
        assert_eq!(report.validated_example_config_count, 1);

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        validate_repo_hygiene_write_allowlist(&repo, &[]);
        validate_repo_hygiene_write(&repo, "examples/local.toml", "local\n");
        let err = super::validate_repo_hygiene_at(&repo, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("untracked root workflow artifacts"));
        assert!(err.contains("examples/local.toml"));

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        validate_repo_hygiene_write_allowlist(&repo, &[]);
        validate_repo_hygiene_write(&repo, "prompts/local.md", "local\n");
        let err = super::validate_repo_hygiene_at(&repo, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("prompts/local.md"));

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        validate_repo_hygiene_write_allowlist(&repo, &[]);
        validate_repo_hygiene_write(&repo, ".gitignore", "examples/*.toml\n");
        validate_repo_hygiene_stage(&repo, &[".gitignore"]);
        validate_repo_hygiene_write(&repo, "examples/ignored.toml", "ignored\n");
        let err = super::validate_repo_hygiene_at(&repo, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("examples/ignored.toml"));

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        validate_repo_hygiene_write(repo.as_path(), DEFAULT_ARTIFACT_ALLOWLIST_PATH, "");
        let err = super::validate_repo_hygiene_at(&repo, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains(
            "artifact allowlist .github/workflow-artifact-allowlist.txt is not tracked by git"
        ));

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        validate_repo_hygiene_write_valid_artifacts(&repo);
        validate_repo_hygiene_write(
            &repo,
            DEFAULT_ARTIFACT_ALLOWLIST_PATH,
            "examples/good.toml\nexamples/missing.toml\nprompts/project.md\n",
        );
        let err = super::validate_repo_hygiene_at(&repo, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unstaged changes"));
        assert!(err.contains(DEFAULT_ARTIFACT_ALLOWLIST_PATH));

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        validate_repo_hygiene_write_valid_artifacts(&repo);
        validate_repo_hygiene_write(&repo, "examples/good.toml", "not toml");
        let err = super::validate_repo_hygiene_at(&repo, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unstaged changes"));
        assert!(err.contains("examples/good.toml"));

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        let external = repo
            .parent()
            .unwrap()
            .join("external-workflow-artifact-allowlist.txt");
        std::fs::write(&external, "").unwrap();
        let err = super::validate_repo_hygiene_at(&repo, Some(&external))
            .unwrap_err()
            .to_string();
        assert!(err.contains("must live under repository root"));

        #[cfg(unix)]
        {
            let (_td, repo) = validate_repo_hygiene_temp_repo();
            validate_repo_hygiene_write_valid_artifacts(&repo);
            let external = repo.parent().unwrap().join("external-allowlist.txt");
            std::fs::write(&external, "examples/good.toml\nprompts/project.md\n").unwrap();
            let symlink_rel = ".github/symlink-allowlist.txt";
            std::os::unix::fs::symlink(&external, repo.join(symlink_rel)).unwrap();
            validate_repo_hygiene_stage(&repo, &[symlink_rel]);
            let err =
                super::validate_repo_hygiene_at(&repo, Some(std::path::Path::new(symlink_rel)))
                    .unwrap_err()
                    .to_string();
            assert!(err.contains("resolves outside repository root"));
        }

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        validate_repo_hygiene_write_valid_artifacts(&repo);
        validate_repo_hygiene_write_allowlist(&repo, &["prompts/project.md"]);
        let err = super::validate_repo_hygiene_at(&repo, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("tracked artifacts missing from allowlist"));
        assert!(err.contains("examples/good.toml"));

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        validate_repo_hygiene_write_valid_artifacts(&repo);
        validate_repo_hygiene_write_allowlist(
            &repo,
            &[
                "examples/good.toml",
                "examples/missing.toml",
                "prompts/project.md",
            ],
        );
        let err = super::validate_repo_hygiene_at(&repo, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("allowlist entries without tracked files"));
        assert!(err.contains("examples/missing.toml"));

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        validate_repo_hygiene_write_allowlist(&repo, &[]);
        validate_repo_hygiene_write(&repo, "examples/sample-input.md", "sample\n");
        super::validate_repo_hygiene_at(&repo, None).unwrap();

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        let err = super::validate_repo_hygiene_at(&repo, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("artifact allowlist not found"));
        assert!(err.contains("repository root"));

        let (_td, repo) = validate_repo_hygiene_temp_repo();
        validate_repo_hygiene_write(&repo, "examples/bad.toml", "not toml");
        validate_repo_hygiene_stage(&repo, &["examples/bad.toml"]);
        validate_repo_hygiene_write_allowlist(&repo, &["examples/bad.toml"]);
        let err = super::validate_repo_hygiene_at(&repo, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("tracked example configs failed validation"));
        assert!(err.contains("examples/bad.toml"));
    }

    #[test]
    fn validate_rejects_invalid_startup_only_sections() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("a2a-bridge.toml");
        let base = "default=\"codex\"\n[[agents]]\nid=\"codex\"\ncmd=\"codex-acp\"\n[server]\n";

        std::fs::write(
            &cfg,
            format!("{base}\n[batch]\nmax_concurrent = 0\n").as_bytes(),
        )
        .unwrap();
        let err = super::validate_config_file(
            &cfg,
            super::ExamplesPolicy::Off,
            &[],
            super::ValidationScope::Startup,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("[batch] max_concurrent"),
            "batch validation should match serve/mcp startup"
        );

        std::fs::write(
            &cfg,
            format!("{base}\n[worktrees]\nenabled = true\n").as_bytes(),
        )
        .unwrap();
        let err = super::validate_config_file(
            &cfg,
            super::ExamplesPolicy::Off,
            &[],
            super::ValidationScope::Startup,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("[worktrees] enabled requires allowed_cwd_root"),
            "worktree validation should match serve/mcp startup"
        );
    }

    #[test]
    fn startup_validation_skips_language_profile_details() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("a2a-bridge.toml");
        std::fs::write(
            &cfg,
            r#"
default = "codex"

[[agents]]
id = "codex"
cmd = "codex-acp"

[server]

[[languages]]
id = "rust"
fetch = "cargo fetch --locked"
warm_cache = "a2a-test-cache"
dep_cache_path = "/cargo"
verify_cache_path = "/verify"
"#,
        )
        .unwrap();

        super::validate_config_file(
            &cfg,
            super::ExamplesPolicy::Off,
            &[],
            super::ValidationScope::Startup,
        )
        .unwrap();
        let err = super::validate_config_file(
            &cfg,
            super::ExamplesPolicy::Off,
            &[],
            super::ValidationScope::Full,
        )
        .unwrap_err();
        assert!(err.to_string().contains("language profiles"));
    }

    #[test]
    fn examples_policy_flags_project_specific_examples() {
        let dir = tempfile::tempdir().unwrap();
        let examples = dir.path().join("examples");
        std::fs::create_dir(&examples).unwrap();
        let cfg = examples.join("project.toml");
        std::fs::write(
            &cfg,
            "command = \"/Users/wesleyjinks/code/slicing/target/release/prism-mcp\"",
        )
        .unwrap();
        let matched = super::has_project_marker_in_examples(
            &cfg,
            "command = \"/Users/wesleyjinks/code/slicing/target/release/prism-mcp\"",
            &["code/slicing".to_string()],
        );
        assert!(
            matched,
            "examples policy should flag project-specific material in examples/"
        );
    }

    #[test]
    fn examples_policy_uses_runtime_examples_path() {
        let dir = tempfile::tempdir().unwrap();
        let examples = dir.path().join("examples");
        std::fs::create_dir(&examples).unwrap();
        let cfg = examples.join("project.toml");
        std::fs::write(&cfg, "command = \"prism-mcp\"\n").unwrap();
        assert!(super::has_project_marker_in_examples(
            &cfg,
            "command = \"prism-mcp\"\n",
            &["prism-mcp".to_string()],
        ));
        let warnings =
            super::examples_policy_warning_for_config(&std::fs::canonicalize(&cfg).unwrap());
        assert!(
            warnings
                .first()
                .is_some_and(|w| w.contains(&examples.to_string_lossy().to_string())),
            "policy warning should derive the examples root from the runtime config path"
        );
    }

    #[test]
    fn examples_policy_rejects_empty_markers() {
        assert!(super::finalize_examples_policy(
            Some(super::ExamplesPolicy::Deny),
            &["".to_string()]
        )
        .is_err());
        assert!(super::finalize_examples_policy(
            Some(super::ExamplesPolicy::Warn),
            &["   ".to_string()]
        )
        .is_err());
    }

    #[test]
    fn examples_policy_explicit_off_does_not_promote_markers() {
        assert_eq!(
            super::finalize_examples_policy(None, &["prism-mcp".to_string()]).unwrap(),
            super::ExamplesPolicy::Warn
        );
        assert_eq!(
            super::finalize_examples_policy(
                Some(super::ExamplesPolicy::Off),
                &["prism-mcp".to_string()]
            )
            .unwrap(),
            super::ExamplesPolicy::Off
        );
    }

    #[test]
    fn examples_policy_scans_resolved_workflow_prompt_files() {
        let dir = tempfile::tempdir().unwrap();
        let examples = dir.path().join("examples");
        let prompts = dir.path().join("prompts");
        std::fs::create_dir(&examples).unwrap();
        std::fs::create_dir(&prompts).unwrap();
        std::fs::write(prompts.join("project.md"), "use prism-mcp here\n").unwrap();
        let cfg = examples.join("project.toml");
        std::fs::write(
            &cfg,
            r#"
default = "codex"

[[agents]]
id = "codex"
cmd = "codex-acp"

[server]

[[workflows]]
id = "review"

[[workflows.nodes]]
id = "n"
agent = "codex"
prompt_file = "../prompts/project.md"
"#,
        )
        .unwrap();

        let err = super::validate_config_file(
            &cfg,
            super::ExamplesPolicy::Deny,
            &["prism-mcp".to_string()],
            super::ValidationScope::Full,
        )
        .unwrap_err();
        assert!(err.to_string().contains("examples policy denied"));
    }

    #[test]
    fn examples_policy_scans_named_prompt_files_even_when_unused() {
        let dir = tempfile::tempdir().unwrap();
        let examples = dir.path().join("examples");
        let prompts = dir.path().join("prompts");
        std::fs::create_dir(&examples).unwrap();
        std::fs::create_dir(&prompts).unwrap();
        std::fs::write(prompts.join("named.md"), "marker: prism-mcp\n").unwrap();
        let cfg = examples.join("project.toml");
        std::fs::write(
            &cfg,
            r#"
default = "codex"

[[agents]]
id = "codex"
cmd = "codex-acp"

[server]

[[prompts]]
id = "project"
file = "../prompts/named.md"
"#,
        )
        .unwrap();

        let err = super::validate_config_file(
            &cfg,
            super::ExamplesPolicy::Deny,
            &["prism-mcp".to_string()],
            super::ValidationScope::Full,
        )
        .unwrap_err();
        assert!(err.to_string().contains("examples policy denied"));
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

    // ===== E8a — prompt CLI (T7..T9) =====

    #[test]
    fn prompt_list_sorts_ids_no_file_io() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("a2a-bridge.toml");
        // `file` points at a MISSING file — `list` must still work (no read).
        std::fs::write(
            &cfg,
            "default=\"codex\"\n[[agents]]\nid=\"codex\"\ncmd=\"codex-acp\"\n\
            [[prompts]]\nid=\"zeta\"\nfile=\"missing.md\"\ndescription=\"z\"\n\
            [[prompts]]\nid=\"alpha\"\ntext=\"hi\"\n[server]\naddr=\"127.0.0.1:8080\"\n",
        )
        .unwrap();
        let out = super::prompt_list_lines(&cfg).unwrap();
        assert_eq!(
            out,
            vec![
                "alpha — (no description)".to_string(),
                "zeta — z".to_string()
            ]
        );
    }

    #[test]
    fn prompt_show_resolves_one_and_errors_on_unknown() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("r.md"), "REVIEW {{input}}\n").unwrap();
        let cfg = dir.path().join("a2a-bridge.toml");
        std::fs::write(
            &cfg,
            "default=\"codex\"\n[[agents]]\nid=\"codex\"\ncmd=\"codex-acp\"\n\
            [[prompts]]\nid=\"rev\"\nfile=\"r.md\"\n[[prompts]]\nid=\"s\"\ntext=\"hi\"\n\
            [server]\naddr=\"127.0.0.1:8080\"\n",
        )
        .unwrap();
        assert_eq!(
            super::prompt_show_text(&cfg, "rev").unwrap(),
            "REVIEW {{input}}\n"
        );
        assert_eq!(super::prompt_show_text(&cfg, "s").unwrap(), "hi");
        let err = super::prompt_show_text(&cfg, "ghost")
            .unwrap_err()
            .to_string();
        assert!(err.contains("ghost") && err.contains("rev")); // unknown + available ids
    }

    #[test]
    fn prompt_cmd_dispatch_help_unknown_sub_and_strict_args() {
        let s = |a: &str| a.to_string();
        assert!(super::prompt_cmd(&[s("--help")]).is_ok());
        assert!(super::prompt_cmd(&[s("bogus")]).is_err());
        assert!(super::prompt_cmd(&[]).is_err()); // missing subcommand
        assert!(super::prompt_cmd(&[s("show"), s("x"), s("--resolved")]).is_err());
        assert!(super::prompt_cmd(&[s("list"), s("--bogusflag")]).is_err());
        assert!(super::prompt_cmd(&[s("list"), s("extra")]).is_err());
        assert!(super::prompt_cmd(&[s("show"), s("a"), s("b")]).is_err());
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

    /// Drive `run_workflow_serve_client` against a wiremock `serve` that returns
    /// a completed SSE stream, and capture the request it POSTs. Returns
    /// `(authorization_present, a2a_version_header, body)`.
    async fn capture_streaming_request(
        context: &str,
        session_cwd: Option<&str>,
    ) -> (bool, Option<String>, serde_json::Value) {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stream_response_sse(terminal_update(
                        a2a::TaskState::Completed,
                    ))),
            )
            .mount(&server)
            .await;
        super::run_workflow_serve_client("wf", "hello", None, &server.uri(), context, session_cwd)
            .await
            .unwrap();
        let reqs = server.received_requests().await.unwrap();
        let req = &reqs[0];
        let auth_present = req.headers.get("authorization").is_some();
        let version = req
            .headers
            .get("a2a-version")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        (auth_present, version, body)
    }

    #[tokio::test]
    async fn serve_client_builds_streaming_message() {
        let (auth_present, version, body) =
            capture_streaming_request("C", Some("/work/repo")).await;
        // Headers: A2A-Version present, Authorization ABSENT (loopback).
        assert_eq!(version.as_deref(), Some("1.0"), "A2A-Version header");
        assert!(!auth_present, "loopback must send no Authorization header");
        // Envelope + message shape (G-C1 allowed delta).
        assert_eq!(body["method"], a2a::methods::SEND_STREAMING_MESSAGE);
        let message = &body["params"]["message"];
        assert_eq!(message["contextId"], "C");
        assert!(
            message["taskId"].as_str().is_some_and(|s| !s.is_empty()),
            "taskId missing from {body}"
        );
        assert_eq!(message["metadata"]["a2a-bridge.skill"], "wf");
        assert_eq!(message["metadata"]["a2a-bridge.cwd"], "/work/repo");
        assert_eq!(message["parts"][0]["text"], "hello");
        // Allowed-delta additions: messageId + role "ROLE_USER".
        assert!(
            message["messageId"].as_str().is_some_and(|s| !s.is_empty()),
            "messageId missing from {message}"
        );
        assert_eq!(message["role"], "ROLE_USER");
        // parts[0] loses `kind: text` (server is lenient).
        assert!(
            message["parts"][0].get("kind").is_none(),
            "parts[0].kind must be absent, got {message}"
        );
    }

    #[tokio::test]
    async fn serve_client_requests_have_distinct_task_ids() {
        let (_a, _v, first) = capture_streaming_request("C", None).await;
        let (_a2, _v2, second) = capture_streaming_request("C", None).await;
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

    // ---- T-C3: run-workflow --serve terminal/artifact/frame characterization ----

    /// A `StatusUpdate(Completed)` that still carries a `message` — under the
    /// CLI's `message.is_none()` gate this is NOT terminal.
    fn completed_with_message(text: &str) -> a2a::StreamResponse {
        a2a::StreamResponse::StatusUpdate(a2a::TaskStatusUpdateEvent {
            task_id: "task-1".to_string(),
            context_id: "C".to_string(),
            status: a2a::TaskStatus {
                state: a2a::TaskState::Completed,
                message: Some(a2a::Message::new(
                    a2a::Role::User,
                    vec![a2a::Part::text(text)],
                )),
                timestamp: None,
            },
            metadata: None,
        })
    }

    #[tokio::test]
    async fn run_workflow_serve_concatenates_artifacts_in_arrival_order() {
        // (a) "A" + "B" + Completed → output "AB".
        let body = format!(
            "{}{}{}",
            stream_response_sse(artifact_update("A")),
            stream_response_sse(artifact_update("B")),
            stream_response_sse(terminal_update(a2a::TaskState::Completed)),
        );
        let (output, result) = run_workflow_with_fake_serve(body).await.unwrap();
        assert!(result.is_ok(), "unexpected error: {result:?}");
        assert_eq!(output, "AB");
    }

    #[tokio::test]
    async fn run_workflow_serve_artifact_then_eof_without_terminal_errors() {
        // (b) artifact + EOF with no terminal status → hard error.
        let body = stream_response_sse(artifact_update("partial"));
        let (_output, result) = run_workflow_with_fake_serve(body).await.unwrap();
        let err = result.unwrap_err();
        assert!(
            err.contains("stream ended without terminal status"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn run_workflow_serve_completed_with_message_is_not_terminal() {
        // (c) Completed WITH a message → not terminal (message.is_none() gate),
        // so the stream ends without a recorded terminal status — same as (b).
        let body = stream_response_sse(completed_with_message("still working"));
        let (_output, result) = run_workflow_with_fake_serve(body).await.unwrap();
        let err = result.unwrap_err();
        assert!(
            err.contains("stream ended without terminal status"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn run_workflow_serve_bad_frame_is_hard_error() {
        // (d) undecodable SSE `data:` payload → hard error, preserved verbatim.
        let body = "data: this is not valid json\n\n".to_string();
        let (_output, result) = run_workflow_with_fake_serve(body).await.unwrap();
        let err = result.unwrap_err();
        assert!(
            err.contains("bad SSE data frame"),
            "unexpected error: {err}"
        );
    }

    // ---- §6#2: `task watch --from` transport (Last-Event-ID + typed method) ----

    async fn task_watch_capture(id: &str, from: Option<i64>) -> serde_json::Value {
        let server = wiremock::MockServer::start().await;
        // One SSE event carrying an `id:` line so the resume path is exercised.
        let frame = format!(
            "id: 7\ndata: {}\n\n",
            serde_json::to_string(&terminal_update(a2a::TaskState::Completed)).unwrap()
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(frame),
            )
            .mount(&server)
            .await;
        super::task_watch_cmd(&server.uri(), id, from)
            .await
            .unwrap();
        let reqs = server.received_requests().await.unwrap();
        let req = &reqs[0];
        let last_event_id = req
            .headers
            .get("last-event-id")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        serde_json::json!({
            "last_event_id": last_event_id,
            "method": body["method"],
            "params": body["params"],
        })
    }

    #[tokio::test]
    async fn task_watch_sends_last_event_id_when_from_set() {
        let captured = task_watch_capture("task-1", Some(7)).await;
        assert_eq!(
            captured["last_event_id"], "7",
            "Last-Event-ID must be sent when --from is set"
        );
        // Uses the typed method constant, not a hardcoded "SubscribeToTask".
        assert_eq!(captured["method"], a2a::methods::SUBSCRIBE_TO_TASK);
        assert_eq!(captured["params"]["id"], "task-1");
    }

    #[tokio::test]
    async fn task_watch_omits_last_event_id_without_from() {
        let captured = task_watch_capture("task-1", None).await;
        assert!(
            captured["last_event_id"].is_null(),
            "Last-Event-ID must be absent without --from, got {captured}"
        );
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
