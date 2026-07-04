// doctor.rs — `a2a-bridge doctor` (wave 3, W3-B): a read-only, advisory preflight.
//
// Contract (spec docs/superpowers/specs/2026-07-03-wave-3-cli-wire.md §W3-B): parse + validate the
// config, then report on the things that most commonly break a first run (agent commands/runtimes,
// api_key_env, sandbox egress, [verify]/[review] infra, the [store] path, MCP servers, the lsp_env
// containerized-MCP-env trap, and configured credential bind-mounts) as `ok | warn | fail` rows with a
// one-line remedy. ZERO filesystem writes, no live egress, no agent/container spawns — every external
// probe is bounded so a wedged runtime is reported, never hung on.
//
// ARCHITECTURE: `run_checks` is a PURE core over already-loaded, plain data (`LoadedConfig`) and a small
// `RuntimeProbes` trait — the ONLY seam that touches the outside world (PATH lookups, bounded
// subprocesses, filesystem stats, env vars). Unit tests inject a fake `RuntimeProbes` and hand-built
// `LoadedConfig` fixtures, so they never touch the real system. `doctor_cmd` is the thin, impure CLI
// wrapper: it resolves the config path, loads + parses the config once (reusing `validate_config_file`
// for check 1, exactly as the spec requires), builds a `LoadedConfig`, and renders the result.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bridge_core::domain::{AgentKind, EgressPolicy, RegistrySnapshot};

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CheckResult {
    pub check: String,
    pub status: CheckStatus,
    pub detail: String,
    pub remedy: String,
}

impl CheckResult {
    fn ok(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            status: CheckStatus::Ok,
            detail: detail.into(),
            remedy: String::new(),
        }
    }
    fn warn(
        check: impl Into<String>,
        detail: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            check: check.into(),
            status: CheckStatus::Warn,
            detail: detail.into(),
            remedy: remedy.into(),
        }
    }
    fn fail(
        check: impl Into<String>,
        detail: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            check: check.into(),
            status: CheckStatus::Fail,
            detail: detail.into(),
            remedy: remedy.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// RuntimeProbes — the only seam that touches the outside world.
// ---------------------------------------------------------------------------

/// Static metadata for a host filesystem path — never mutates, never creates (doctor is read-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathStat {
    pub exists: bool,
    pub is_dir: bool,
    /// Advisory: no write permission for the current user (best-effort; `Permissions::readonly()`).
    pub readonly: bool,
}

impl PathStat {
    const ABSENT: Self = Self {
        exists: false,
        is_dir: false,
        readonly: false,
    };
}

/// Every external probe `run_checks` needs, injectable so unit tests never touch the real system.
/// Implementations MUST be bounded — a hung/wedged external process must resolve to `false` within a
/// hard timeout, never block indefinitely (see `RealProbes`'s `bounded_probe_ok`).
pub trait RuntimeProbes {
    /// `cmd` resolves to an executable file — either as a literal path (absolute or containing `/`) or
    /// by searching `$PATH` for a bare name.
    fn which_on_path(&self, cmd: &str) -> bool;
    /// `<runtime> info` (or equivalent) exits 0 within a bound.
    fn runtime_responds(&self, runtime: &str) -> bool;
    /// `<runtime> network inspect <network>` exits 0 within a bound.
    fn network_exists(&self, runtime: &str, network: &str) -> bool;
    /// `<runtime> image inspect <image>` exits 0 within a bound. Advisory only — a missing image just
    /// means the runtime will pull it on first use (or fail offline), never a hard requirement.
    fn image_exists(&self, runtime: &str, image: &str) -> bool;
    /// Stat a host path. Never creates, never follows into a write probe (TOCTOU/mutating — cut per
    /// the spec's adversarial review).
    fn path_stat(&self, path: &Path) -> PathStat;
    /// Whether `name` is set (present, regardless of value) in the current process environment.
    fn env_var_set(&self, name: &str) -> bool;
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounded `<program> <args...>` — true iff the process exits 0 within `timeout`. A wedged/slow process
/// is killed and treated as "not found", so `doctor` can never hang on a half-started daemon. This
/// duplicates (rather than reuses) `main.rs`'s `runtime_responds` bounded-poll pattern because it needs
/// an arbitrary arg vector (network/image name), not the fixed `info` subcommand; `runtime_responds`
/// itself IS reused as-is below (see `RealProbes::runtime_responds`).
fn bounded_probe_ok(program: &str, args: &[&str], timeout: Duration) -> bool {
    let mut child = match std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

fn which_on_path_impl(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }
    if cmd.contains('/') {
        return is_executable_file(Path::new(cmd));
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| is_executable_file(&dir.join(cmd)))
}

fn path_stat_impl(path: &Path) -> PathStat {
    match std::fs::metadata(path) {
        Ok(m) => PathStat {
            exists: true,
            is_dir: m.is_dir(),
            readonly: m.permissions().readonly(),
        },
        Err(_) => PathStat::ABSENT,
    }
}

/// The production `RuntimeProbes` — every method is bounded (nothing here can hang `doctor`).
pub struct RealProbes;

impl RuntimeProbes for RealProbes {
    fn which_on_path(&self, cmd: &str) -> bool {
        which_on_path_impl(cmd)
    }
    fn runtime_responds(&self, runtime: &str) -> bool {
        // Reused verbatim from main.rs (same bounded `<runtime> info` pattern preflight_runtimes uses).
        crate::runtime_responds(runtime)
    }
    fn network_exists(&self, runtime: &str, network: &str) -> bool {
        bounded_probe_ok(runtime, &["network", "inspect", network], PROBE_TIMEOUT)
    }
    fn image_exists(&self, runtime: &str, image: &str) -> bool {
        bounded_probe_ok(runtime, &["image", "inspect", image], PROBE_TIMEOUT)
    }
    fn path_stat(&self, path: &Path) -> PathStat {
        path_stat_impl(path)
    }
    fn env_var_set(&self, name: &str) -> bool {
        std::env::var(name).is_ok()
    }
}

// ---------------------------------------------------------------------------
// LoadedConfig — plain, already-parsed data. Building this is the ONLY place doctor touches the
// filesystem outside of `probes`; `run_checks` itself never reads a file.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOkSummary {
    pub agent_count: usize,
    pub workflow_count: usize,
    pub prompt_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyCheckInput {
    /// Resolved (defaulted) runtime — mirrors `SandboxConfig::runtime()`'s "docker" default so the
    /// probed value matches what would actually run.
    pub runtime: String,
    pub image: String,
    pub locked_network: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCheckInput {
    pub slice_cmd: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageCheckInput {
    pub id: String,
    pub lsp_env_keys: Vec<String>,
}

/// Everything `run_checks` needs, already loaded/parsed by the caller (`doctor_cmd`/`load_config`) so
/// the check core itself does no file I/O — only `probes` touches the outside world.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// Outcome of check 1 (`validate_config_file`, reused verbatim). `Err` short-circuits every other
    /// check — an invalid config leaves nothing safe to inspect.
    pub config_check: Result<ConfigOkSummary, String>,
    /// `None` iff `config_check` is `Err`.
    pub snapshot: Option<RegistrySnapshot>,
    pub verify: Option<Result<VerifyCheckInput, String>>,
    pub review: Option<Result<ReviewCheckInput, String>>,
    /// Resolved config-dir-relative, exactly as `serve` resolves `[store].path` (main.rs's inline
    /// resolution ~:5955-5961). `None` = no `[store]` configured (in-memory task store).
    pub store_path: Option<PathBuf>,
    pub languages: Vec<LanguageCheckInput>,
}

impl LoadedConfig {
    fn config_error(msg: impl Into<String>) -> Self {
        Self {
            config_check: Err(msg.into()),
            snapshot: None,
            verify: None,
            review: None,
            store_path: None,
            languages: Vec::new(),
        }
    }
}

/// Extract the doctor-owned, plain-data view of an already-parsed config. Reads verify/review/store/
/// languages BEFORE `into_snapshot` (which consumes `cfg.agents`/`cfg.default`/etc.) — pure data
/// transformation, no I/O.
fn build_loaded_config(
    cfg: crate::config::RegistryConfig,
    config_dir: &Path,
    config_check: Result<ConfigOkSummary, String>,
) -> Result<LoadedConfig, String> {
    let verify = cfg.verify.as_ref().map(|v| {
        v.to_config().map_err(|e| e.to_string()).map(|vc| {
            let runtime = vc
                .runtime
                .clone()
                .unwrap_or_else(|| bridge_core::domain::DEFAULT_RUNTIME.to_string());
            let locked_network = match &vc.egress {
                EgressPolicy::Locked { network, .. } => Some(network.clone()),
                EgressPolicy::Open => None,
            };
            VerifyCheckInput {
                runtime,
                image: vc.image.clone(),
                locked_network,
            }
        })
    });
    let review = cfg.review.as_ref().map(|r| {
        r.to_config()
            .map_err(|e| e.to_string())
            .map(|rc| ReviewCheckInput {
                slice_cmd: rc.slice_cmd,
            })
    });
    let store_path = cfg.store.as_ref().map(|s| {
        let p = Path::new(&s.path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            config_dir.join(p)
        }
    });
    let languages = cfg
        .languages
        .iter()
        .map(|l| LanguageCheckInput {
            id: l.id.clone(),
            lsp_env_keys: l
                .lsp_env
                .as_ref()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
        })
        .collect();

    let snapshot = cfg.into_snapshot().map_err(|e| e.to_string())?;

    Ok(LoadedConfig {
        config_check,
        snapshot: Some(snapshot),
        verify,
        review,
        store_path,
        languages,
    })
}

/// Load + parse the config at `config_path` into a `LoadedConfig`. Check 1 is produced by reusing
/// `validate_config_file` verbatim (per spec); checks 2-9 need the parsed `RegistryConfig` again — a
/// second small parse of the same file (doctor is an occasional manual preflight, not a hot path).
fn load_config(config_path: &Path) -> LoadedConfig {
    let config_check = crate::validate_config_file(
        config_path,
        crate::ExamplesPolicy::Off,
        &[],
        crate::ValidationScope::Full,
    )
    .map(|r| ConfigOkSummary {
        agent_count: r.agent_count,
        workflow_count: r.workflow_count,
        prompt_count: r.prompt_count,
    })
    .map_err(|e| e.to_string());

    if config_check.is_err() {
        return LoadedConfig {
            config_check,
            ..LoadedConfig::config_error(String::new())
        };
    }

    let config_dir = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let raw = match std::fs::read_to_string(config_path) {
        Ok(r) => r,
        Err(e) => return LoadedConfig::config_error(format!("doctor: read {config_path:?}: {e}")),
    };
    let cfg = match crate::config::RegistryConfig::parse(&raw) {
        Ok(c) => c,
        Err(e) => return LoadedConfig::config_error(format!("doctor: {e}")),
    };
    match build_loaded_config(cfg, &config_dir, config_check) {
        Ok(lc) => lc,
        Err(e) => LoadedConfig::config_error(e),
    }
}

// ---------------------------------------------------------------------------
// run_checks — the pure core.
// ---------------------------------------------------------------------------

/// Run all 9 doctor checks against an already-loaded config. PURE: every side-effecting operation goes
/// through `probes`; nothing here reads a file, spawns a process, or writes anything.
pub fn run_checks(cfg: &LoadedConfig, probes: &dyn RuntimeProbes) -> Vec<CheckResult> {
    let mut out = Vec::new();

    // Check 1: config parses + registry validates.
    match &cfg.config_check {
        Ok(s) => out.push(CheckResult::ok(
            "config",
            format!(
                "parsed OK — {} agent(s), {} workflow(s), {} named prompt(s)",
                s.agent_count, s.workflow_count, s.prompt_count
            ),
        )),
        Err(e) => {
            out.push(CheckResult::fail(
                "config",
                e.clone(),
                "fix the config error above, then re-run `a2a-bridge doctor` (or `a2a-bridge validate --config <path>` for full diagnostics)",
            ));
            return out; // nothing else is safe to inspect without a valid snapshot.
        }
    }

    let Some(snapshot) = &cfg.snapshot else {
        // Defensive: `build_loaded_config` always pairs an Ok config_check with Some(snapshot).
        return out;
    };

    check_agent_commands(snapshot, probes, &mut out); // check 2
    check_api_key_env(snapshot, probes, &mut out); // check 3
    check_sandbox_egress(snapshot, probes, &mut out); // check 4
    check_verify(&cfg.verify, probes, &mut out); // check 5
    check_store(&cfg.store_path, probes, &mut out); // check 6
    check_mcp_servers(snapshot, probes, &mut out); // check 7 (mcp half)
    check_lsp_env(&cfg.languages, &mut out); // check 7 (lsp_env lint half)
    check_review_slice_cmd(&cfg.review, probes, &mut out); // check 8
    check_creds(snapshot, probes, &mut out); // check 9

    out
}

/// Check 2 — host-vs-sandbox command semantics. `Api`-kind entries have no `cmd`/runtime to check here
/// (covered entirely by check 3). A sandboxed entry's spawned program is the container RUNTIME
/// (`sb.runtime()`), never the inner `cmd` — probing the inner cmd on the HOST would be meaningless
/// (it may not even exist outside the image) and is explicitly out of scope.
fn check_agent_commands(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        if entry.kind == AgentKind::Api {
            continue;
        }
        let id = entry.id.as_str();
        match &entry.sandbox {
            Some(sb) => {
                let runtime = sb.runtime();
                let check = format!("agent:{id}:runtime");
                if probes.runtime_responds(runtime) {
                    out.push(CheckResult::ok(
                        check,
                        format!("sandbox runtime {runtime:?} responds"),
                    ));
                } else {
                    out.push(CheckResult::fail(
                        check,
                        format!("sandboxed agent {id:?} runtime {runtime:?} did not respond to `{runtime} info`"),
                        format!("install/start {runtime} (for podman: `podman machine start`), or fix [agents.sandbox].runtime for {id}"),
                    ));
                }
            }
            None => {
                let Some(cmd) = entry.cmd.as_deref() else {
                    continue;
                };
                let check = format!("agent:{id}:cmd");
                let on_path = probes.which_on_path(cmd);
                let allowed = snapshot.allowed_cmds.iter().any(|a| a == cmd);
                if on_path && allowed {
                    out.push(CheckResult::ok(
                        check,
                        format!("{cmd:?} on PATH and allowed"),
                    ));
                } else {
                    let mut reasons = Vec::new();
                    if !on_path {
                        reasons.push(format!("{cmd:?} not found on PATH"));
                    }
                    if !allowed {
                        reasons.push(format!("{cmd:?} not in [registry].allowed_cmds"));
                    }
                    out.push(CheckResult::fail(
                        check,
                        reasons.join("; "),
                        format!("install {cmd:?} on PATH and add it to [registry].allowed_cmds"),
                    ));
                }
            }
        }
    }
}

/// Check 3 — `kind="api"`: warn/fail ONLY when `api_key_env` is configured AND unset (unset is a
/// deterministic auth failure, so this is a `fail` like the other missing-required-thing checks; a
/// no-auth local backend with no `api_key_env` at all is valid and reports `ok`).
fn check_api_key_env(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        if entry.kind != AgentKind::Api {
            continue;
        }
        let id = entry.id.as_str();
        let check = format!("agent:{id}:api-key-env");
        match &entry.api_key_env {
            None => out.push(CheckResult::ok(
                check,
                "no api_key_env configured (no-auth backend)",
            )),
            Some(name) => {
                if probes.env_var_set(name) {
                    out.push(CheckResult::ok(check, format!("${name} is set")));
                } else {
                    out.push(CheckResult::fail(
                        check,
                        format!("${name} is configured as api_key_env but is not set in this process's environment"),
                        format!("export {name}=<token> before starting a2a-bridge (or remove api_key_env from agent {id} if it needs no auth)"),
                    ));
                }
            }
        }
    }
}

/// Check 4 — sandbox egress: a `Locked` network must resolve (compose_sandbox would otherwise spawn
/// against a nonexistent `--network`, a hard failure — `fail`); the image is advisory (the runtime may
/// pull it on demand — `warn`). `Open` egress has no locked network to inspect.
fn check_sandbox_egress(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        let Some(sb) = &entry.sandbox else {
            continue;
        };
        let id = entry.id.as_str();
        match &sb.egress {
            EgressPolicy::Locked { network, .. } => {
                let runtime = sb.runtime();
                let net_check = format!("agent:{id}:sandbox-network");
                if probes.network_exists(runtime, network) {
                    out.push(CheckResult::ok(
                        net_check,
                        format!("locked network {network:?} found"),
                    ));
                } else {
                    out.push(CheckResult::fail(
                        net_check,
                        format!("locked network {network:?} not found via `{runtime} network inspect`"),
                        format!("create it: `{runtime} network create {network}` (or fix [agents.sandbox].network for {id})"),
                    ));
                }
                let img_check = format!("agent:{id}:sandbox-image");
                if probes.image_exists(runtime, &sb.image) {
                    out.push(CheckResult::ok(
                        img_check,
                        format!("image {:?} present locally", sb.image),
                    ));
                } else {
                    out.push(CheckResult::warn(
                        img_check,
                        format!("image {:?} not present locally (advisory — the runtime may pull on demand)", sb.image),
                        format!("pre-pull with `{runtime} pull {}` to avoid a first-run delay or an offline failure", sb.image),
                    ));
                }
            }
            EgressPolicy::Open => {
                out.push(CheckResult::ok(
                    format!("agent:{id}:sandbox-egress"),
                    "egress open (no locked network to inspect)",
                ));
            }
        }
    }
}

/// Check 5 — `[verify]` preflight (added per review): its own runtime/image/locked-network, exactly like
/// check 4's sandbox egress (a broken verify runtime or missing toolchain image must surface — it runs
/// unattended after every `implement` commit).
fn check_verify(
    verify: &Option<Result<VerifyCheckInput, String>>,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    match verify {
        None => out.push(CheckResult::ok(
            "verify",
            "[verify] not configured (skipped)",
        )),
        Some(Err(e)) => out.push(CheckResult::fail(
            "verify",
            e.clone(),
            "fix the [verify] block",
        )),
        Some(Ok(v)) => {
            if probes.runtime_responds(&v.runtime) {
                out.push(CheckResult::ok(
                    "verify:runtime",
                    format!("runtime {:?} responds", v.runtime),
                ));
            } else {
                out.push(CheckResult::fail(
                    "verify:runtime",
                    format!("[verify] runtime {:?} did not respond to `{} info`", v.runtime, v.runtime),
                    format!("install/start {} (for podman: `podman machine start`), or fix [verify].runtime", v.runtime),
                ));
            }
            if probes.image_exists(&v.runtime, &v.image) {
                out.push(CheckResult::ok(
                    "verify:image",
                    format!("image {:?} present locally", v.image),
                ));
            } else {
                out.push(CheckResult::warn(
                    "verify:image",
                    format!("[verify] image {:?} not present locally (advisory — may pull on demand)", v.image),
                    format!("pre-pull with `{} pull {}` to avoid a first-run delay or an offline failure", v.runtime, v.image),
                ));
            }
            if let Some(network) = &v.locked_network {
                if probes.network_exists(&v.runtime, network) {
                    out.push(CheckResult::ok(
                        "verify:network",
                        format!("locked network {network:?} found"),
                    ));
                } else {
                    out.push(CheckResult::fail(
                        "verify:network",
                        format!("[verify] locked network {network:?} not found via `{} network inspect`", v.runtime),
                        format!("create it: `{} network create {network}` (or fix [verify].network)", v.runtime),
                    ));
                }
            }
        }
    }
}

/// Check 6 — store: resolve `[store].path` exactly as `serve` does (config-dir-relative); parent must
/// exist and be a dir (`fail` otherwise — `SqliteStore::open` cannot create a parent dir), with
/// permission metadata as an ADVISORY `warn` (per spec — no write probe, TOCTOU/mutating). No `[store]`
/// configured is the valid in-memory-store case, reported `ok`.
fn check_store(
    store_path: &Option<PathBuf>,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    match store_path {
        None => out.push(CheckResult::ok(
            "store",
            "no [store] configured; task history is in-memory only (not durable across restarts)",
        )),
        Some(resolved) => {
            let parent = resolved.parent().unwrap_or(resolved.as_path());
            let st = probes.path_stat(parent);
            if !st.exists || !st.is_dir {
                out.push(CheckResult::fail(
                    "store",
                    format!("[store] parent directory {parent:?} does not exist"),
                    format!("create it: `mkdir -p {parent:?}`"),
                ));
            } else if st.readonly {
                out.push(CheckResult::warn(
                    "store",
                    format!("[store] parent directory {parent:?} exists but is read-only"),
                    "serve needs write access there to create/open the sqlite store",
                ));
            } else {
                out.push(CheckResult::ok(
                    "store",
                    format!("[store] parent directory {parent:?} exists and is writable"),
                ));
            }
        }
    }
}

/// Check 7 (MCP half) — host-delivered `[[agents.mcp]]` commands must be on PATH; container-delivered
/// servers are baked into the sandbox image, so they're informational-only (`ok`, "not checked").
fn check_mcp_servers(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        let id = entry.id.as_str();
        for server in &entry.mcp {
            let check = format!("mcp:{id}:{}", server.name);
            if entry.sandbox.is_none() {
                if probes.which_on_path(&server.command) {
                    out.push(CheckResult::ok(
                        check,
                        format!("{:?} on PATH (host-delivered)", server.command),
                    ));
                } else {
                    out.push(CheckResult::fail(
                        check,
                        format!("MCP server command {:?} not found on PATH", server.command),
                        format!(
                            "install {:?} on PATH (host-delivered MCP for agent {id})",
                            server.command
                        ),
                    ));
                }
            } else {
                out.push(CheckResult::ok(
                    check,
                    "container-delivered (in-image, not checked)",
                ));
            }
        }
    }
}

/// The `lsp_env` keys documented (docs/containerized-mcp-env-trap.md) as load-bearing for a language's
/// in-container MCP server — a containerized MCP subprocess gets a STRIPPED env, so anything the server
/// (or a proxy/shim it depends on, e.g. rustup) reads at startup MUST be forwarded via `lsp_env`. Only
/// the two documented, root-caused traps are linted; other language ids have no known required key.
fn known_required_lsp_env_keys(lang_id: &str) -> &'static [&'static str] {
    match lang_id {
        "rust" => &["RUSTUP_HOME"],
        "python" => &["LSP_MCP_PYTHON_PATH"],
        _ => &[],
    }
}

/// Check 7 (lsp_env lint half, added per review) — static, no probe: entries whose in-container MCP
/// relies on env documented as load-bearing must set it via `lsp_env`, not inherit (it won't).
fn check_lsp_env(languages: &[LanguageCheckInput], out: &mut Vec<CheckResult>) {
    for lang in languages {
        let required = known_required_lsp_env_keys(&lang.id);
        if required.is_empty() {
            continue; // no documented trap for this language id.
        }
        let check = format!("lsp_env:{}", lang.id);
        let missing: Vec<&str> = required
            .iter()
            .copied()
            .filter(|k| !lang.lsp_env_keys.iter().any(|have| have == k))
            .collect();
        if missing.is_empty() {
            out.push(CheckResult::ok(
                check,
                format!("required key(s) {required:?} present"),
            ));
        } else {
            out.push(CheckResult::warn(
                check,
                format!(
                    "missing recommended lsp_env key(s) {missing:?} — a containerized MCP subprocess gets a \
                     stripped env, not the image's ENV"
                ),
                format!(
                    "add {missing:?} to [[languages]] id={:?} lsp_env (see docs/containerized-mcp-env-trap.md)",
                    lang.id
                ),
            ));
        }
    }
}

/// Check 8 (added per review) — `[review].slice_cmd` (default points at prism, config.rs:743-745); a
/// missing binary silently degrades review depth to non-sliced context, so `warn`, never `fail`.
fn check_review_slice_cmd(
    review: &Option<Result<ReviewCheckInput, String>>,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    match review {
        None => out.push(CheckResult::ok(
            "review:slice_cmd",
            "[review] not configured (skipped)",
        )),
        Some(Err(e)) => out.push(CheckResult::fail(
            "review:slice_cmd",
            e.clone(),
            "fix the [review] block",
        )),
        Some(Ok(r)) => {
            let cmd = r.slice_cmd.to_string_lossy().into_owned();
            if probes.which_on_path(&cmd) {
                out.push(CheckResult::ok(
                    "review:slice_cmd",
                    format!("{cmd:?} resolves"),
                ));
            } else {
                out.push(CheckResult::warn(
                    "review:slice_cmd",
                    format!("[review].slice_cmd {cmd:?} not found — review depth silently degrades to non-sliced context"),
                    format!("install prism at {cmd:?} (or point [review].slice_cmd elsewhere)"),
                ));
            }
        }
    }
}

fn expand_tilde(s: &str) -> String {
    match s.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => s.to_string(),
        },
        None => s.to_string(),
    }
}

/// A bind-mount host source is an absolute (or `~`-relative) path; a bare name (e.g.
/// `a2a-kiro-data:/root/.local/share`) is a named/managed volume, not a host path — nothing to stat.
fn is_bind_mount_host(host_seg: &str) -> bool {
    host_seg.starts_with('/') || host_seg.starts_with('~')
}

/// Check 9 — creds: configured bind-mount cred sources (the sandbox's `volumes`, e.g. a mounted
/// `.credentials.json`/`auth.json`) exist as host files; named volumes are skipped (informational — not
/// a host path). STATIC only (no freshness/expiry check — cut per review as TOCTOU/mutating-adjacent).
/// NOTE: item 9's "env vars named by config are set" clause is the SAME fact as check 3's
/// `api_key_env` check (the only config surface naming an env var) — folded there, not duplicated here.
fn check_creds(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        let Some(sb) = &entry.sandbox else {
            continue;
        };
        let id = entry.id.as_str();
        for (i, vol) in sb.volumes.iter().enumerate() {
            let host_seg = vol.split(':').next().unwrap_or("");
            let check = format!("creds:{id}:{i}");
            if !is_bind_mount_host(host_seg) {
                out.push(CheckResult::ok(
                    check,
                    format!("named volume {host_seg:?} (not a host path, skipped)"),
                ));
                continue;
            }
            let host_path = expand_tilde(host_seg);
            let st = probes.path_stat(Path::new(&host_path));
            if st.exists {
                out.push(CheckResult::ok(
                    check,
                    format!("bind-mount source {host_path:?} exists"),
                ));
            } else {
                out.push(CheckResult::fail(
                    check,
                    format!("bind-mount source {host_path:?} does not exist"),
                    format!("create/copy the credential file at {host_path:?} for agent {id} (see docs/containerized-agents.md)"),
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Aligned-column plain text (this codebase has no ANSI/color dependency anywhere — verified via grep —
/// so doctor stays plain, matching every other CLI table here, e.g. `containers list`).
pub fn render_text(results: &[CheckResult]) -> String {
    let check_w = results
        .iter()
        .map(|r| r.check.len())
        .max()
        .unwrap_or(0)
        .max("CHECK".len());
    let status_w = "STATUS".len();
    let mut out = String::new();
    out.push_str(&format!(
        "{:<check_w$}  {:<status_w$}  DETAIL\n",
        "CHECK", "STATUS"
    ));
    let (mut ok, mut warn, mut fail) = (0usize, 0usize, 0usize);
    for r in results {
        match r.status {
            CheckStatus::Ok => ok += 1,
            CheckStatus::Warn => warn += 1,
            CheckStatus::Fail => fail += 1,
        }
        out.push_str(&format!(
            "{:<check_w$}  {:<status_w$}  {}\n",
            r.check,
            r.status.as_str(),
            r.detail
        ));
        if r.status != CheckStatus::Ok && !r.remedy.is_empty() {
            let indent = check_w + status_w + 4;
            out.push_str(&format!("{:indent$}remedy: {}\n", "", r.remedy));
        }
    }
    out.push_str(&format!("\n{ok} ok, {warn} warn, {fail} fail\n"));
    out
}

// ---------------------------------------------------------------------------
// CLI wrapper
// ---------------------------------------------------------------------------

/// `a2a-bridge doctor [--config <path>] [--json]`. Exit 0 unless at least one check is `fail`.
pub fn doctor_cmd(args: &[String]) -> Result<(), crate::BoxError> {
    let mut explicit_config: Option<PathBuf> = None;
    let mut json = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            // Belt-and-suspenders: the dispatcher already intercepts `doctor --help`/`-h` before this
            // function is ever called (see `dispatcher_help`); this mirrors other subcommands' own
            // internal check for defense-in-depth (e.g. `validate_cmd`, `prompt_cmd`).
            "--help" | "-h" => {
                println!("{}", crate::DOCTOR_USAGE);
                return Ok(());
            }
            "--config" => {
                explicit_config = Some(PathBuf::from(
                    it.next().ok_or("doctor: --config requires a <path>")?,
                ));
            }
            "--json" => json = true,
            other => {
                return Err(format!(
                    "doctor: unknown argument {other:?}\n{}",
                    crate::DOCTOR_USAGE
                )
                .into());
            }
        }
    }

    let config_path = crate::require_config_path(explicit_config)?;
    let loaded = load_config(&config_path);
    let probes = RealProbes;
    let results = run_checks(&loaded, &probes);

    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        print!("{}", render_text(&results));
    }

    // Print first, decide exit code from content second — a `fail` row must still be fully rendered,
    // not swallowed behind a generic top-level "error: ..." line.
    if results.iter().any(|r| r.status == CheckStatus::Fail) {
        std::process::exit(1);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{AgentEntry, MountAccess, SandboxConfig};
    use bridge_core::ids::AgentId;
    use bridge_core::mcp::McpServerSpec;
    use std::collections::{HashMap, HashSet};

    // ---- fixtures ----

    fn acp_entry(id: &str, cmd: &str) -> AgentEntry {
        AgentEntry {
            id: AgentId::parse(id).unwrap(),
            cmd: Some(cmd.to_string()),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind: AgentKind::Acp,
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
            extensions: Default::default(),
        }
    }

    fn api_entry(id: &str, api_key_env: Option<&str>) -> AgentEntry {
        let mut e = acp_entry(id, "");
        e.cmd = None;
        e.kind = AgentKind::Api;
        e.base_url = Some("http://localhost:11434/v1".to_string());
        e.api_key_env = api_key_env.map(str::to_string);
        e
    }

    fn locked_sandbox(image: &str, network: &str, volumes: Vec<String>) -> SandboxConfig {
        SandboxConfig {
            runtime: None,
            image: image.to_string(),
            mount: "/work".to_string(),
            access: MountAccess::Ro,
            egress: EgressPolicy::Locked {
                network: network.to_string(),
                proxy: "http://proxy:8080".to_string(),
                no_proxy: None,
            },
            volumes,
        }
    }

    fn snapshot(
        default: &str,
        entries: Vec<AgentEntry>,
        allowed_cmds: Vec<&str>,
    ) -> RegistrySnapshot {
        RegistrySnapshot {
            default: AgentId::parse(default).unwrap(),
            entries,
            allowed_cmds: allowed_cmds.into_iter().map(str::to_string).collect(),
        }
    }

    fn ok_summary() -> ConfigOkSummary {
        ConfigOkSummary {
            agent_count: 1,
            workflow_count: 0,
            prompt_count: 0,
        }
    }

    fn base_loaded(snap: RegistrySnapshot) -> LoadedConfig {
        LoadedConfig {
            config_check: Ok(ok_summary()),
            snapshot: Some(snap),
            verify: None,
            review: None,
            store_path: None,
            languages: Vec::new(),
        }
    }

    #[derive(Default)]
    struct FakeProbes {
        on_path: HashSet<String>,
        responsive_runtimes: HashSet<String>,
        networks: HashSet<String>,
        images: HashSet<String>,
        env_vars: HashSet<String>,
        paths: HashMap<PathBuf, PathStat>,
    }

    impl FakeProbes {
        fn new() -> Self {
            Self::default()
        }
        fn allow_path(mut self, cmd: &str) -> Self {
            self.on_path.insert(cmd.to_string());
            self
        }
        fn allow_runtime(mut self, rt: &str) -> Self {
            self.responsive_runtimes.insert(rt.to_string());
            self
        }
        fn allow_network(mut self, n: &str) -> Self {
            self.networks.insert(n.to_string());
            self
        }
        fn allow_image(mut self, i: &str) -> Self {
            self.images.insert(i.to_string());
            self
        }
        fn allow_env(mut self, e: &str) -> Self {
            self.env_vars.insert(e.to_string());
            self
        }
        fn with_path(mut self, p: &str, st: PathStat) -> Self {
            self.paths.insert(PathBuf::from(p), st);
            self
        }
        fn with_dir(self, p: &str, writable: bool) -> Self {
            self.with_path(
                p,
                PathStat {
                    exists: true,
                    is_dir: true,
                    readonly: !writable,
                },
            )
        }
        fn with_file(self, p: &str) -> Self {
            self.with_path(
                p,
                PathStat {
                    exists: true,
                    is_dir: false,
                    readonly: false,
                },
            )
        }
    }

    impl RuntimeProbes for FakeProbes {
        fn which_on_path(&self, cmd: &str) -> bool {
            self.on_path.contains(cmd)
        }
        fn runtime_responds(&self, runtime: &str) -> bool {
            self.responsive_runtimes.contains(runtime)
        }
        fn network_exists(&self, _runtime: &str, network: &str) -> bool {
            self.networks.contains(network)
        }
        fn image_exists(&self, _runtime: &str, image: &str) -> bool {
            self.images.contains(image)
        }
        fn path_stat(&self, path: &Path) -> PathStat {
            self.paths.get(path).copied().unwrap_or(PathStat::ABSENT)
        }
        fn env_var_set(&self, name: &str) -> bool {
            self.env_vars.contains(name)
        }
    }

    fn find<'a>(results: &'a [CheckResult], check: &str) -> &'a CheckResult {
        results
            .iter()
            .find(|r| r.check == check)
            .unwrap_or_else(|| panic!("no check named {check:?} in {results:#?}"))
    }

    // ---- check 1: config ----

    #[test]
    fn config_error_short_circuits_to_a_single_fail_row() {
        let cfg = LoadedConfig::config_error("boom: bad toml");
        let results = run_checks(&cfg, &FakeProbes::new());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].check, "config");
        assert_eq!(results[0].status, CheckStatus::Fail);
        assert_eq!(results[0].detail, "boom: bad toml");
        assert!(!results[0].remedy.is_empty());
    }

    #[test]
    fn config_ok_reports_counts() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "config");
        assert_eq!(row.status, CheckStatus::Ok);
        assert!(row.detail.contains('1'));
    }

    // ---- check 2: host-vs-sandbox command semantics ----

    #[test]
    fn host_entry_cmd_missing_fails() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        let probes = FakeProbes::new(); // "codex-acp" NOT on path
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "agent:codex:cmd");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(row.detail.contains("not found on PATH"), "{}", row.detail);
        assert!(!row.remedy.is_empty());
    }

    #[test]
    fn host_entry_cmd_present_but_not_allowlisted_fails() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["other"],
        ));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "agent:codex:cmd");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(row.detail.contains("allowed_cmds"), "{}", row.detail);
    }

    #[test]
    fn sandboxed_entry_missing_runtime_fails_with_remedy() {
        let mut kiro = acp_entry("kiro", "kiro-cli");
        kiro.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("kiro", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new(); // docker not responsive
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "agent:kiro:runtime");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(row.detail.contains("docker"), "{}", row.detail);
        assert!(!row.remedy.is_empty());
        // The inner cmd ("kiro-cli") must NOT be probed on the host — no such check exists.
        assert!(results.iter().all(|r| r.check != "agent:kiro:cmd"));
    }

    #[test]
    fn sandboxed_entry_responsive_runtime_is_ok() {
        let mut kiro = acp_entry("kiro", "kiro-cli");
        kiro.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("kiro", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "agent:kiro:runtime").status, CheckStatus::Ok);
    }

    // ---- check 3: api_key_env ----

    #[test]
    fn api_entry_unset_configured_env_fails() {
        let cfg = base_loaded(snapshot(
            "m",
            vec![api_entry("m", Some("OPENAI_API_KEY"))],
            vec![],
        ));
        let probes = FakeProbes::new(); // OPENAI_API_KEY not set
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "agent:m:api-key-env");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(row.detail.contains("OPENAI_API_KEY"), "{}", row.detail);
        assert!(!row.remedy.is_empty());
    }

    #[test]
    fn api_entry_set_configured_env_is_ok() {
        let cfg = base_loaded(snapshot(
            "m",
            vec![api_entry("m", Some("OPENAI_API_KEY"))],
            vec![],
        ));
        let probes = FakeProbes::new().allow_env("OPENAI_API_KEY");
        let results = run_checks(&cfg, &probes);
        assert_eq!(
            find(&results, "agent:m:api-key-env").status,
            CheckStatus::Ok
        );
    }

    #[test]
    fn api_entry_no_configured_env_is_ok_no_auth_backend() {
        let cfg = base_loaded(snapshot("m", vec![api_entry("m", None)], vec![]));
        let results = run_checks(&cfg, &FakeProbes::new());
        assert_eq!(
            find(&results, "agent:m:api-key-env").status,
            CheckStatus::Ok
        );
    }

    // ---- check 4: sandbox egress ----

    #[test]
    fn locked_egress_missing_network_fails() {
        let mut kiro = acp_entry("kiro", "kiro-cli");
        kiro.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("kiro", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_image("reader:latest");
        let results = run_checks(&cfg, &probes);
        assert_eq!(
            find(&results, "agent:kiro:sandbox-network").status,
            CheckStatus::Fail
        );
    }

    #[test]
    fn locked_egress_missing_image_warns_advisory() {
        let mut kiro = acp_entry("kiro", "kiro-cli");
        kiro.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("kiro", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "agent:kiro:sandbox-image");
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(row.detail.contains("may pull on demand"), "{}", row.detail);
    }

    #[test]
    fn open_egress_reports_ok_with_no_network_check() {
        let mut e = acp_entry("impl", "codex-acp");
        e.sandbox = Some(SandboxConfig {
            runtime: None,
            image: "img".to_string(),
            mount: "/work".to_string(),
            access: MountAccess::Rw,
            egress: EgressPolicy::Open,
            volumes: vec![],
        });
        let cfg = base_loaded(snapshot("impl", vec![e], vec!["docker"]));
        let probes = FakeProbes::new().allow_runtime("docker");
        let results = run_checks(&cfg, &probes);
        assert_eq!(
            find(&results, "agent:impl:sandbox-egress").status,
            CheckStatus::Ok
        );
        assert!(results
            .iter()
            .all(|r| r.check != "agent:impl:sandbox-network"));
    }

    // ---- check 5: [verify] ----

    #[test]
    fn verify_not_configured_is_ok() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "verify").status, CheckStatus::Ok);
    }

    #[test]
    fn missing_verify_image_warns_advisory() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.verify = Some(Ok(VerifyCheckInput {
            runtime: "docker".to_string(),
            image: "toolchain:rust".to_string(),
            locked_network: Some("a2a-net".to_string()),
        }));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .allow_runtime("docker")
            .allow_network("a2a-net"); // image NOT allowed
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "verify:image");
        assert_eq!(row.status, CheckStatus::Warn);
        assert_eq!(find(&results, "verify:runtime").status, CheckStatus::Ok);
        assert_eq!(find(&results, "verify:network").status, CheckStatus::Ok);
    }

    #[test]
    fn verify_runtime_not_responding_fails() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.verify = Some(Ok(VerifyCheckInput {
            runtime: "podman".to_string(),
            image: "toolchain:rust".to_string(),
            locked_network: None,
        }));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .allow_image("toolchain:rust");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "verify:runtime").status, CheckStatus::Fail);
        assert!(results.iter().all(|r| r.check != "verify:network")); // no locked network configured
    }

    // ---- check 6: store ----

    #[test]
    fn store_not_configured_is_ok() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "store").status, CheckStatus::Ok);
    }

    #[test]
    fn store_parent_missing_fails() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.store_path = Some(PathBuf::from("/does/not/exist/tasks.db"));
        let probes = FakeProbes::new().allow_path("codex-acp"); // no path_stat entry registered
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "store");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(!row.remedy.is_empty());
    }

    #[test]
    fn store_parent_readonly_warns() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.store_path = Some(PathBuf::from("/store/tasks.db"));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .with_dir("/store", false);
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "store").status, CheckStatus::Warn);
    }

    #[test]
    fn store_parent_present_and_writable_is_ok() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.store_path = Some(PathBuf::from("/store/tasks.db"));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .with_dir("/store", true);
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "store").status, CheckStatus::Ok);
    }

    // ---- check 7: mcp servers + lsp_env lint ----

    #[test]
    fn mcp_host_delivered_missing_on_path_fails() {
        let mut e = acp_entry("codex", "codex-acp");
        e.mcp = vec![McpServerSpec {
            name: "lsp".to_string(),
            command: "/usr/local/bin/lsp-mcp".to_string(),
            args: vec![],
            env: vec![],
        }];
        let cfg = base_loaded(snapshot("codex", vec![e], vec!["codex-acp"]));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "mcp:codex:lsp").status, CheckStatus::Fail);
    }

    #[test]
    fn mcp_container_delivered_is_informational_ok() {
        let mut e = acp_entry("kiro", "kiro-cli");
        e.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        e.mcp = vec![McpServerSpec {
            name: "lsp".to_string(),
            command: "/usr/local/bin/lsp-mcp".to_string(),
            args: vec![],
            env: vec![],
        }];
        let cfg = base_loaded(snapshot("kiro", vec![e], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "mcp:kiro:lsp");
        assert_eq!(row.status, CheckStatus::Ok);
        assert!(row.detail.contains("in-image"), "{}", row.detail);
    }

    #[test]
    fn lsp_env_missing_required_key_warns() {
        let cfg = LoadedConfig {
            languages: vec![LanguageCheckInput {
                id: "rust".to_string(),
                lsp_env_keys: vec!["CARGO_HOME".to_string()], // RUSTUP_HOME missing
            }],
            ..base_loaded(snapshot(
                "codex",
                vec![acp_entry("codex", "codex-acp")],
                vec!["codex-acp"],
            ))
        };
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "lsp_env:rust");
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(row.detail.contains("RUSTUP_HOME"), "{}", row.detail);
        assert!(row.remedy.contains("containerized-mcp-env-trap.md"));
    }

    #[test]
    fn lsp_env_with_required_key_is_ok() {
        let cfg = LoadedConfig {
            languages: vec![LanguageCheckInput {
                id: "rust".to_string(),
                lsp_env_keys: vec!["CARGO_HOME".to_string(), "RUSTUP_HOME".to_string()],
            }],
            ..base_loaded(snapshot(
                "codex",
                vec![acp_entry("codex", "codex-acp")],
                vec!["codex-acp"],
            ))
        };
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "lsp_env:rust").status, CheckStatus::Ok);
    }

    #[test]
    fn lsp_env_unknown_language_id_has_no_lint_row() {
        let cfg = LoadedConfig {
            languages: vec![LanguageCheckInput {
                id: "go".to_string(),
                lsp_env_keys: vec![],
            }],
            ..base_loaded(snapshot(
                "codex",
                vec![acp_entry("codex", "codex-acp")],
                vec!["codex-acp"],
            ))
        };
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert!(results.iter().all(|r| r.check != "lsp_env:go"));
    }

    // ---- check 8: review slice_cmd ----

    #[test]
    fn review_not_configured_is_ok() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "review:slice_cmd").status, CheckStatus::Ok);
    }

    #[test]
    fn slice_cmd_missing_warns() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.review = Some(Ok(ReviewCheckInput {
            slice_cmd: PathBuf::from("prism"),
        }));
        let probes = FakeProbes::new().allow_path("codex-acp"); // "prism" NOT on path
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "review:slice_cmd");
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(!row.remedy.is_empty());
    }

    #[test]
    fn slice_cmd_present_is_ok() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.review = Some(Ok(ReviewCheckInput {
            slice_cmd: PathBuf::from("prism"),
        }));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .allow_path("prism");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "review:slice_cmd").status, CheckStatus::Ok);
    }

    // ---- check 9: creds (bind-mount volumes) ----

    #[test]
    fn bind_mount_cred_missing_fails() {
        let mut kiro = acp_entry("codex-impl", "codex-acp");
        kiro.sandbox = Some(locked_sandbox(
            "toolchain:rust",
            "a2a-net",
            vec!["/Users/x/.config/a2a-creds/codex/auth.json:/root/.codex/auth.json".to_string()],
        ));
        let cfg = base_loaded(snapshot("codex-impl", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("toolchain:rust"); // cred file NOT registered
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "creds:codex-impl:0");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(!row.remedy.is_empty());
    }

    #[test]
    fn bind_mount_cred_present_is_ok() {
        let mut kiro = acp_entry("codex-impl", "codex-acp");
        kiro.sandbox = Some(locked_sandbox(
            "toolchain:rust",
            "a2a-net",
            vec!["/Users/x/.config/a2a-creds/codex/auth.json:/root/.codex/auth.json".to_string()],
        ));
        let cfg = base_loaded(snapshot("codex-impl", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("toolchain:rust")
            .with_file("/Users/x/.config/a2a-creds/codex/auth.json");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "creds:codex-impl:0").status, CheckStatus::Ok);
    }

    #[test]
    fn named_volume_is_skipped_not_a_host_path() {
        let mut kiro = acp_entry("kiro", "kiro-cli");
        kiro.sandbox = Some(locked_sandbox(
            "reader:latest",
            "a2a-net",
            vec!["a2a-kiro-data:/root/.local/share".to_string()],
        ));
        let cfg = base_loaded(snapshot("kiro", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "creds:kiro:0");
        assert_eq!(row.status, CheckStatus::Ok);
        assert!(row.detail.contains("not a host path"), "{}", row.detail);
    }

    // ---- end-to-end: all-ok config ----

    #[test]
    fn all_ok_config_produces_zero_warn_or_fail() {
        let host = acp_entry("codex", "codex-acp");
        let mut sandboxed = acp_entry("kiro", "kiro-cli");
        sandboxed.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let mut cfg = base_loaded(snapshot(
            "kiro",
            vec![host, sandboxed],
            vec!["codex-acp", "docker"],
        ));
        cfg.verify = Some(Ok(VerifyCheckInput {
            runtime: "docker".to_string(),
            image: "toolchain:rust".to_string(),
            locked_network: Some("a2a-net".to_string()),
        }));
        cfg.review = Some(Ok(ReviewCheckInput {
            slice_cmd: PathBuf::from("prism"),
        }));
        cfg.store_path = Some(PathBuf::from("/store/tasks.db"));
        cfg.languages = vec![LanguageCheckInput {
            id: "rust".to_string(),
            lsp_env_keys: vec!["CARGO_HOME".to_string(), "RUSTUP_HOME".to_string()],
        }];

        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .allow_path("prism")
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest")
            .allow_image("toolchain:rust")
            .with_dir("/store", true);

        let results = run_checks(&cfg, &probes);
        assert!(!results.is_empty());
        let bad: Vec<&CheckResult> = results
            .iter()
            .filter(|r| r.status != CheckStatus::Ok)
            .collect();
        assert!(bad.is_empty(), "expected all-ok, got: {bad:#?}");
    }

    // ---- JSON shape ----

    #[test]
    fn json_output_shape_stable() {
        let results = vec![
            CheckResult::ok("config", "parsed OK"),
            CheckResult::warn("verify:image", "missing", "pre-pull it"),
            CheckResult::fail("agent:x:cmd", "not found", "install it"),
        ];
        let v = serde_json::to_value(&results).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        for item in arr {
            let obj = item.as_object().unwrap();
            let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(keys, vec!["check", "detail", "remedy", "status"]);
        }
        assert_eq!(arr[0]["status"], "ok");
        assert_eq!(arr[1]["status"], "warn");
        assert_eq!(arr[2]["status"], "fail");
    }

    // ---- loader: store path resolution ----

    #[test]
    fn build_loaded_config_resolves_store_path_relative_to_config_dir() {
        let toml = "default = \"codex\"\n[server]\n[[agents]]\nid = \"codex\"\ncmd = \"codex-acp\"\n[store]\npath = \"data/tasks.db\"\n";
        let cfg = crate::config::RegistryConfig::parse(toml).unwrap();
        let loaded = build_loaded_config(cfg, Path::new("/repo/cfgdir"), Ok(ok_summary())).unwrap();
        assert_eq!(
            loaded.store_path,
            Some(PathBuf::from("/repo/cfgdir/data/tasks.db"))
        );
    }

    #[test]
    fn build_loaded_config_keeps_absolute_store_path_unchanged() {
        let toml = "default = \"codex\"\n[server]\n[[agents]]\nid = \"codex\"\ncmd = \"codex-acp\"\n[store]\npath = \"/abs/tasks.db\"\n";
        let cfg = crate::config::RegistryConfig::parse(toml).unwrap();
        let loaded = build_loaded_config(cfg, Path::new("/repo/cfgdir"), Ok(ok_summary())).unwrap();
        assert_eq!(loaded.store_path, Some(PathBuf::from("/abs/tasks.db")));
    }
}
