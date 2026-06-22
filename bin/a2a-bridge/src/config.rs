// config.rs — TOML configuration for the a2a-bridge binary (spec §8, Task 15).

use std::collections::BTreeMap;
use std::fmt;

use bridge_core::domain::{AgentEntry, AgentKind, Effort, RegistrySnapshot};
use bridge_core::ids::AgentId;

/// Unified parse error covering TOML parse failures and missing env-var references.
#[derive(Debug)]
pub enum ConfigError {
    Toml(toml::de::Error),
    MissingEnvVar(String),
    /// Invalid registry config value (e.g. unknown effort level, empty agent id).
    /// Wired to main in Task 12.
    Registry(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Toml(e) => write!(f, "TOML parse error: {e}"),
            ConfigError::MissingEnvVar(v) => write!(f, "env var ${{{v}}} not set"),
            ConfigError::Registry(msg) => write!(f, "registry config: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Toml(e) => Some(e),
            ConfigError::MissingEnvVar(_) | ConfigError::Registry(_) => None,
        }
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        ConfigError::Toml(e)
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_addr")]
    pub addr: String,
    #[serde(default = "default_warm_idle_ttl_secs")]
    pub warm_idle_ttl_secs: u64,
    /// Advisory pre-task warn when carried context usage >= this window fraction in (0,1]. None = off. [Slice 2]
    #[serde(default)]
    pub warm_usage_warn_fraction: Option<f64>,
    #[serde(default)]
    pub compact_summarize_timeout_secs: Option<u64>,
    #[serde(default)]
    pub permission_policy: Option<String>,
    #[serde(default)]
    pub permission_timeout_ms: Option<u64>,
}

#[derive(Debug, serde::Deserialize)]
pub struct StoreConfig {
    pub path: String,
    #[serde(default)]
    pub resume_attempt_cap: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
pub struct DelegationConfig {
    pub peer_url: String,
    pub auth: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_addr() -> String {
    "127.0.0.1:8080".into()
}

fn default_warm_idle_ttl_secs() -> u64 {
    1800
}

fn default_timeout_secs() -> u64 {
    60
}

/// Expand `${VAR_NAME}` placeholders in `s` using `std::env::var`.
/// Returns `Err(ConfigError::MissingEnvVar)` if any referenced variable is unset.
fn expand_env(s: &str) -> Result<String, ConfigError> {
    let mut result = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let after_brace = &rest[start + 2..];
        let end = after_brace
            .find('}')
            .ok_or_else(|| ConfigError::MissingEnvVar("<unterminated ${...}>".into()))?;
        let var_name = &after_brace[..end];
        let value = std::env::var(var_name)
            .map_err(|_| ConfigError::MissingEnvVar(var_name.to_string()))?;
        result.push_str(&value);
        rest = &after_brace[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}

// ---------------------------------------------------------------------------
// Multi-agent registry config (Task 7 / Increment 3b).
// Parses a TOML with `[[agents]]` array + optional `[registry]` section.
// Main is rewired to use this in Task 12.
// ---------------------------------------------------------------------------

/// Top-level TOML structure for the multi-agent bridge config.
#[derive(Debug, serde::Deserialize)]
pub struct RegistryConfig {
    pub default: String,
    #[serde(default)]
    pub registry: Option<RegistrySection>,
    #[serde(default)]
    pub agents: Vec<AgentEntryToml>,
    pub server: ServerConfig,
    #[serde(default)]
    pub delegation: Option<DelegationConfig>,
    #[serde(default)]
    pub store: Option<StoreConfig>,
    #[serde(default)]
    pub workflows: Vec<WorkflowToml>,
    #[serde(default)]
    pub languages: Vec<LanguageToml>,
    /// Sentinel for a top-level `[watchdog]` table: the watchdog is PER-AGENT only
    /// (`[agents.watchdog]`), so a top-level table would be silently ignored. Present →
    /// rejected in `into_snapshot` with a clear "use [agents.watchdog]" error.
    #[serde(default)]
    pub watchdog: Option<toml::Value>,
    /// Global root path that gates which per-request cwds are allowed (later tasks).
    /// Absent → no global root restriction.
    #[serde(default)]
    pub allowed_cwd_root: Option<String>,
    /// `[verify]` (Slice B2b-2): the build+test verify run after `implement` commits. Absent → skipped.
    #[serde(default)]
    pub verify: Option<VerifyToml>,
    /// `[review]` (Slice B2b-3a): the review-the-diff workflow run after `implement` commits. Absent → skipped.
    #[serde(default)]
    pub review: Option<ReviewToml>,
    /// `[implement]` (Slice B2b-3b): the review→tweak loop config. Absent → `LoopConfig::default()`.
    #[serde(default)]
    pub implement: Option<ImplementToml>,
    /// `[merge]` (ADR-0027): merge hand-off target + operator identity override. Absent → defaults.
    #[serde(default)]
    pub merge: Option<MergeToml>,
}

#[derive(Debug, serde::Deserialize)]
pub struct WorkflowToml {
    pub id: String,
    #[serde(default)]
    pub nodes: Vec<WorkflowNodeToml>,
}

#[derive(Debug, serde::Deserialize)]
pub struct WorkflowNodeToml {
    pub id: String,
    pub agent: String,
    pub prompt_file: String,
    #[serde(default)]
    pub inputs: Vec<String>,
}

/// `[registry]` section — optional; controls which cmds are allowed.
#[derive(Debug, serde::Deserialize)]
pub struct RegistrySection {
    #[serde(default)]
    pub allowed_cmds: Vec<String>,
}

/// One entry in the `[[agents]]` array, as parsed from TOML.
/// String fields are converted to typed domain values in `into_snapshot`.
#[derive(Debug, serde::Deserialize)]
pub struct AgentEntryToml {
    pub id: String,
    /// Required for `kind="acp"`; absent for non-process kinds (e.g. `Api`).
    #[serde(default)]
    pub cmd: Option<String>,
    /// OpenAI-compatible base URL; required for `kind="api"`.
    #[serde(default)]
    pub base_url: Option<String>,
    /// NAME of an env var holding a bearer token for `kind="api"` (never the secret).
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Parsed to `AgentKind` in `into_snapshot`; "acp" (default).
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub model_provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Parsed to `Effort` in `into_snapshot`; valid values: minimal/low/medium/high/xhigh/max.
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Static ACP session cwd for this agent (distinct from any host process cwd).
    /// When absent falls back to `cwd` then `"."` at mint time.
    #[serde(default)]
    pub session_cwd: Option<String>,
    /// The enforced `[sandbox]` block (B1). Converted to `SandboxConfig` + S0/S2-checked in `into_snapshot`.
    #[serde(default)]
    pub sandbox: Option<SandboxToml>,
    /// Optional per-agent E9 watchdog. Absent means disabled.
    #[serde(default)]
    pub watchdog: Option<WatchdogToml>,
    #[serde(default)]
    pub auth_method: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// `[[agents.mcp]]` — MCP servers to offer this agent (ADR-0028). Converted + validated in `into_snapshot`.
    #[serde(default)]
    pub mcp: Vec<McpToml>,
    /// Override the auto-detected MCP delivery channel: `acp` | `codex_native` | `kiro_native`.
    #[serde(default)]
    pub mcp_delivery: Option<String>,
    #[serde(default)]
    pub extensions: BTreeMap<String, toml::Value>,
}

/// `[[agents.mcp]]` — one MCP server offered to an agent. `args`/`env` values may contain `{cwd}`
/// (the session repo); `command` must be a literal path (no `{...}`).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpToml {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<EnvToml>,
}

/// `[[agents.mcp.env]]` — a name/value env pair for an MCP server (value may contain `{cwd}`).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvToml {
    pub name: String,
    pub value: String,
}

/// Convert + validate `[[agents.mcp]]` into domain `McpServerSpec`s: non-empty/unique names, a
/// brace-free `command`, and `{cwd}`-only templating in args/env values (ADR-0028).
/// A TOML bare key (`A-Za-z0-9_-`, non-empty) — usable unquoted in a dotted `-c mcp_servers.<k>.*`
/// path. MCP server + env names must satisfy this so the codex `-c` override paths are well-formed.
fn is_toml_bare_key(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn build_mcp_specs(
    mcp: &[McpToml],
    agent_id: &str,
) -> Result<Vec<bridge_core::mcp::McpServerSpec>, ConfigError> {
    use bridge_core::mcp::{validate_cwd_template, McpServerSpec};
    let err = |m: String| ConfigError::Registry(format!("agent {agent_id:?}: {m}"));
    let mut names = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(mcp.len());
    for m in mcp {
        if m.name.trim().is_empty() {
            return Err(err("mcp.name must be non-empty".into()));
        }
        if !is_toml_bare_key(&m.name) {
            return Err(err(format!(
                "mcp name {:?} must be a bare key (A-Za-z0-9_-) — it forms a `mcp_servers.<name>` config path",
                m.name
            )));
        }
        if !names.insert(m.name.clone()) {
            return Err(err(format!("duplicate mcp name {:?}", m.name)));
        }
        if m.command.trim().is_empty() {
            return Err(err(format!("mcp {:?}: command must be non-empty", m.name)));
        }
        if m.command.contains('{') || m.command.contains('}') {
            return Err(err(format!(
                "mcp {:?}: command must be a literal path (no `{{...}}`)",
                m.name
            )));
        }
        for a in &m.args {
            validate_cwd_template(a).map_err(|e| err(format!("mcp {:?} arg: {e}", m.name)))?;
        }
        let mut env_names = std::collections::HashSet::new();
        let mut env = Vec::with_capacity(m.env.len());
        for e in &m.env {
            if e.name.trim().is_empty() {
                return Err(err(format!("mcp {:?}: env name must be non-empty", m.name)));
            }
            if !is_toml_bare_key(&e.name) {
                return Err(err(format!(
                    "mcp {:?}: env name {:?} must be a bare key (A-Za-z0-9_-)",
                    m.name, e.name
                )));
            }
            if !env_names.insert(e.name.clone()) {
                return Err(err(format!(
                    "mcp {:?}: duplicate env name {:?}",
                    m.name, e.name
                )));
            }
            validate_cwd_template(&e.value)
                .map_err(|x| err(format!("mcp {:?} env {:?}: {x}", m.name, e.name)))?;
            env.push((e.name.clone(), e.value.clone()));
        }
        out.push(McpServerSpec {
            name: m.name.clone(),
            command: m.command.clone(),
            args: m.args.clone(),
            env,
        });
    }
    Ok(out)
}

/// Resolve the MCP delivery channel: explicit `mcp_delivery` override wins; else auto-detect from the
/// `cmd` basename. Only required when the agent actually has MCP servers (`has_mcp`).
fn resolve_mcp_delivery(
    explicit: Option<&str>,
    cmd: Option<&str>,
    has_mcp: bool,
    agent_id: &str,
) -> Result<bridge_core::mcp::McpDelivery, ConfigError> {
    use bridge_core::mcp::McpDelivery;
    if let Some(s) = explicit {
        return match s {
            "acp" => Ok(McpDelivery::Acp),
            "codex_native" => Ok(McpDelivery::CodexNative),
            "kiro_native" => Ok(McpDelivery::KiroNative),
            other => Err(ConfigError::Registry(format!(
                "agent {agent_id:?}: invalid mcp_delivery {other:?} (acp|codex_native|kiro_native)"
            ))),
        };
    }
    let base = cmd
        .and_then(|c| std::path::Path::new(c).file_name())
        .and_then(|s| s.to_str());
    match base {
        Some("claude-agent-acp") => Ok(McpDelivery::Acp),
        Some("codex-acp") => Ok(McpDelivery::CodexNative),
        Some("kiro-cli") => Ok(McpDelivery::KiroNative),
        _ if has_mcp => Err(ConfigError::Registry(format!(
            "agent {agent_id:?}: cannot auto-detect mcp_delivery from cmd {cmd:?}; set mcp_delivery explicitly"
        ))),
        _ => Ok(McpDelivery::Acp),
    }
}

/// `[agents.sandbox]` TOML mirror. Flat for ergonomics; converted to the typed (data-carrying)
/// `EgressPolicy` in `into_snapshot` (which rejects `locked` without `network`+`proxy`).
#[derive(Debug, serde::Deserialize)]
pub struct SandboxToml {
    #[serde(default)]
    pub runtime: Option<String>,
    pub image: String,
    pub mount: String,
    pub access: String, // "ro" | "rw"
    pub egress: String, // "locked" | "open"
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub no_proxy: Option<String>,
    #[serde(default)]
    pub volumes: Vec<String>,
}

/// `[agents.watchdog]` TOML mirror. Durations are positive seconds and are converted to
/// [`bridge_core::domain::WatchdogConfig`] in `into_snapshot`.
#[derive(Debug, serde::Deserialize)]
pub struct WatchdogToml {
    pub idle_timeout_secs: u64,
    pub hard_wall_clock_secs: u64,
}

fn default_gate() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
pub struct LanguageVerifyToml {
    pub name: String,
    pub cmd: String,
    #[serde(default = "default_gate")]
    pub gate: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct LanguageToml {
    pub id: String,
    pub fetch: String,
    pub fetch_env: Option<std::collections::BTreeMap<String, String>>,
    pub warm_cache: String,
    pub dep_cache_path: String,
    pub verify_cache_path: String,
    pub lsp_env: Option<std::collections::BTreeMap<String, String>>,
    pub verify_env: Option<std::collections::BTreeMap<String, String>>,
    pub image: Option<String>,
    #[serde(default)]
    pub verify: Vec<LanguageVerifyToml>,
}

/// `[verify]` (Slice B2b-2): the build+test verify the `implement` subcommand runs after the commit.
/// Egress reuses the shared `parse_egress_fields` invariant (locked ⇒ network+proxy).
#[derive(Debug, serde::Deserialize)]
pub struct VerifyToml {
    #[serde(default)]
    pub runtime: Option<String>,
    pub image: String,
    pub cache: String,
    pub egress: String,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub no_proxy: Option<String>,
    /// REMOVED — moved to `[[languages.verify]]`. Kept ONLY to reject legacy configs loudly
    /// (no deny_unknown_fields). `Option<toml::Value>` detects PRESENCE without reading any fields.
    pub commands: Option<toml::Value>,
}

/// Parsed `[verify]`: verify infrastructure + a validated egress policy.
#[derive(Debug, Clone)]
pub struct VerifyConfig {
    pub runtime: Option<String>,
    pub image: String,
    pub cache: String,
    pub egress: bridge_core::domain::EgressPolicy,
}

impl VerifyToml {
    pub fn to_config(&self) -> Result<VerifyConfig, ConfigError> {
        let egress = parse_egress_fields(&self.egress, &self.network, &self.proxy, &self.no_proxy)?;
        Ok(VerifyConfig {
            runtime: self.runtime.clone(),
            image: self.image.clone(),
            cache: self.cache.clone(),
            egress,
        })
    }
}

/// Gate the resolved `[verify]` runtime against the snapshot's allowlist. PURE — call it once after
/// `into_snapshot()` at each `implement` site with `&snapshot.allowed_cmds`. Only an `Ok` config is
/// checked; a prior `Err` (e.g. empty commands) and `None` pass through untouched (so `None` stays
/// `VerifyOutcome::NotConfigured`, never becomes `ConfigError`). The `"docker"` default is applied HERE
/// to mirror [`bridge_core::domain::SandboxConfig::runtime`] — a defaulted `[verify].runtime` otherwise
/// only resolves to `"docker"` later in `compose_sandbox`, so the gate must apply the same default to
/// check the value that will actually run. A disallowed runtime becomes `ConfigError`, which flows into
/// the existing `VerifyOutcome::ConfigError` path (no container spawns).
pub fn gate_verify_runtime(
    verify_cfg: Option<Result<VerifyConfig, ConfigError>>,
    allowed_cmds: &[String],
) -> Option<Result<VerifyConfig, ConfigError>> {
    match verify_cfg {
        Some(Ok(mut vc)) => {
            // Resolve the default from the SINGLE source so the gate and `compose_sandbox` can't disagree.
            let rt = vc
                .runtime
                .as_deref()
                .unwrap_or(bridge_core::domain::DEFAULT_RUNTIME)
                .to_string();
            if allowed_cmds.contains(&rt) {
                // NORMALIZE: make the resolved runtime explicit so no downstream consumer re-defaults
                // (the verify runner + the preflight read this exact value).
                vc.runtime = Some(rt);
                Some(Ok(vc))
            } else {
                Some(Err(ConfigError::Registry(format!(
                    "verify runtime not allowed: {rt:?} — add it to [registry].allowed_cmds or set [verify].runtime"
                ))))
            }
        }
        other => other,
    }
}

fn default_review_workflow() -> String {
    "implement-review".to_string()
}

fn default_slice_cmd() -> String {
    "~/code/slicing/target/release/prism".to_string()
}

fn default_slice_timeout_secs() -> u64 {
    60
}

fn default_slice_max_bytes() -> usize {
    200_000
}

fn default_light_max_lines() -> usize {
    15
}

fn default_light_max_files() -> usize {
    2
}

fn default_thorough_min_lines() -> usize {
    150
}

fn default_thorough_min_files() -> usize {
    6
}

fn default_depth_str() -> String {
    "auto".to_string()
}

/// `[review]` (Slice B2b-3a): the review-the-diff workflow run after `implement` commits + verifies.
/// Only NAMES a workflow id (model is an agent-level property); absent → review skipped.
#[derive(Debug, serde::Deserialize)]
pub struct ReviewToml {
    #[serde(default = "default_review_workflow")]
    pub workflow: String,
    #[serde(default)]
    pub max_output_bytes: Option<usize>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default = "default_slice_cmd")]
    pub slice_cmd: String,
    #[serde(default = "default_slice_timeout_secs")]
    pub slice_timeout_secs: u64,
    #[serde(default = "default_slice_max_bytes")]
    pub slice_max_bytes: usize,
    #[serde(default = "default_light_max_lines")]
    pub light_max_lines: usize,
    #[serde(default = "default_light_max_files")]
    pub light_max_files: usize,
    #[serde(default = "default_thorough_min_lines")]
    pub thorough_min_lines: usize,
    #[serde(default = "default_thorough_min_files")]
    pub thorough_min_files: usize,
    #[serde(default = "default_depth_str")]
    pub default_depth: String,
}

/// Parsed `[review]`: the workflow id is parsed (validated) HERE, pre-commit, so the post-commit lookup is
/// infallible (a malformed id surfaces as a soft `ConfigError`, never an abort after the commit).
#[derive(Debug, Clone)]
pub struct ReviewConfig {
    pub workflow: bridge_core::ids::WorkflowId,
    pub max_output_bytes: usize,
    pub timeout: std::time::Duration,
    pub slice_cmd: std::path::PathBuf,
    pub slice_timeout: std::time::Duration,
    pub slice_max_bytes: usize,
    pub light_max_lines: usize,
    pub light_max_files: usize,
    pub thorough_min_lines: usize,
    pub thorough_min_files: usize,
    pub default_depth: crate::review::Depth,
}

fn shellexpand_tilde(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => p.to_string(),
        },
        None => p.to_string(),
    }
}

impl ReviewToml {
    pub fn to_config(&self) -> Result<ReviewConfig, ConfigError> {
        let workflow = bridge_core::ids::WorkflowId::parse(self.workflow.clone())
            .map_err(|e| ConfigError::Registry(format!("[review] workflow id: {e:?}")))?;
        let max_output_bytes = self
            .max_output_bytes
            .filter(|&n| n > 0)
            .unwrap_or(16 * 1024);
        let timeout = std::time::Duration::from_secs(self.timeout_secs.unwrap_or(300));
        if self.light_max_lines == 0 || self.light_max_files == 0 {
            return Err(ConfigError::Registry(
                "[review] light_max_lines/light_max_files must be > 0".into(),
            ));
        }
        if self.thorough_min_lines <= self.light_max_lines
            || self.thorough_min_files <= self.light_max_files
        {
            return Err(ConfigError::Registry(
                "[review] thorough_min_lines/thorough_min_files must be > light_max_lines/light_max_files".into(),
            ));
        }
        let default_depth = crate::review::Depth::parse_flag(&self.default_depth)
            .map_err(|e| ConfigError::Registry(format!("[review] default_depth: {e}")))?;
        if self.slice_timeout_secs == 0 || self.slice_max_bytes == 0 {
            return Err(ConfigError::Registry(
                "[review] slice_timeout_secs/slice_max_bytes must be > 0".into(),
            ));
        }
        let slice_cmd = std::path::PathBuf::from(shellexpand_tilde(&self.slice_cmd));
        Ok(ReviewConfig {
            workflow,
            max_output_bytes,
            timeout,
            slice_cmd,
            slice_timeout: std::time::Duration::from_secs(self.slice_timeout_secs),
            slice_max_bytes: self.slice_max_bytes,
            light_max_lines: self.light_max_lines,
            light_max_files: self.light_max_files,
            thorough_min_lines: self.thorough_min_lines,
            thorough_min_files: self.thorough_min_files,
            default_depth,
        })
    }
}

/// `[implement]` (Slice B2b-3b): bounds + names the fix workflow for the review→tweak loop.
#[derive(Debug, serde::Deserialize)]
pub struct ImplementToml {
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub fix_workflow: Option<String>,
    #[serde(default)]
    pub max_session_respawns: Option<u32>,
}

/// Parsed `[implement]`: a validated max + a parsed fix-workflow id (so the post-commit lookup is a soft
/// `FixUnavailable`, never an abort). A malformed block is fail-loud PRE-clone (resolved before the clone).
#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub max_attempts: u32,
    pub fix_workflow: bridge_core::ids::WorkflowId,
    pub max_session_respawns: u32,
}

fn default_fix_workflow_id() -> bridge_core::ids::WorkflowId {
    bridge_core::ids::WorkflowId::parse("implement-fix").expect("static id is valid")
}

const IMPLEMENT_HARD_MAX: u32 = 10;
pub const RESPAWN_HARD_MAX: u32 = 20;

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            fix_workflow: default_fix_workflow_id(),
            max_session_respawns: 3,
        }
    }
}

impl ImplementToml {
    pub fn to_config(&self) -> Result<LoopConfig, ConfigError> {
        let max_attempts = match self.max_attempts {
            None => 3,
            Some(0) => {
                return Err(ConfigError::Registry(
                    "[implement] max_attempts must be >= 1".into(),
                ));
            }
            Some(n) if n > IMPLEMENT_HARD_MAX => {
                eprintln!(
                    "[implement] max_attempts {n} > {IMPLEMENT_HARD_MAX}; clamping to {IMPLEMENT_HARD_MAX}"
                );
                IMPLEMENT_HARD_MAX
            }
            Some(n) => n,
        };
        let fix_workflow = match &self.fix_workflow {
            Some(s) => bridge_core::ids::WorkflowId::parse(s.clone()).map_err(|e| {
                ConfigError::Registry(format!("[implement] fix_workflow id: {e:?}"))
            })?,
            None => default_fix_workflow_id(),
        };
        let max_session_respawns = match self.max_session_respawns {
            None => 3,
            Some(0) => 0, // explicit opt-out: disables in-process warm-session respawns.
            Some(n) if n > RESPAWN_HARD_MAX => {
                eprintln!(
                    "[implement] max_session_respawns {n} > {RESPAWN_HARD_MAX}; clamping to {RESPAWN_HARD_MAX}"
                );
                RESPAWN_HARD_MAX
            }
            Some(n) => n,
        };
        Ok(LoopConfig {
            max_attempts,
            fix_workflow,
            max_session_respawns,
        })
    }
}

/// `[merge]` (ADR-0027) raw TOML: target branch + optional operator identity override. No env expansion
/// (merge takes literal strings); unknown keys ignored (matching the rest of `RegistryConfig`).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MergeToml {
    pub target_ref: Option<String>,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
}

/// Validated `[merge]` config.
#[derive(Debug, Clone)]
pub struct MergeConfig {
    pub target_ref: Option<String>,
    pub author: Option<crate::merge::OperatorIdent>,
}

impl MergeToml {
    /// Fail-loud validation (mirrors `ImplementToml::to_config`): non-empty `target_ref`; identity is
    /// both-or-neither (`author_name` XOR `author_email` → error).
    pub fn to_config(&self) -> Result<MergeConfig, ConfigError> {
        if let Some(t) = &self.target_ref {
            if t.trim().is_empty() {
                return Err(ConfigError::Registry(
                    "[merge].target_ref must be non-empty".into(),
                ));
            }
        }
        let author = match (&self.author_name, &self.author_email) {
            (Some(n), Some(e)) => Some(crate::merge::OperatorIdent {
                name: n.clone(),
                email: e.clone(),
            }),
            (None, None) => None,
            _ => {
                return Err(ConfigError::Registry(
                    "[merge] author_name and author_email must BOTH be set or both omitted".into(),
                ))
            }
        };
        Ok(MergeConfig {
            target_ref: self.target_ref.clone(),
            author,
        })
    }
}

fn parse_access(s: &str) -> Result<bridge_core::domain::MountAccess, ConfigError> {
    use bridge_core::domain::MountAccess;
    match s.to_ascii_lowercase().as_str() {
        "ro" => Ok(MountAccess::Ro),
        "rw" => Ok(MountAccess::Rw),
        other => Err(ConfigError::Registry(format!(
            "invalid sandbox access: {other:?} (expected ro|rw)"
        ))),
    }
}

/// The locked-vs-open invariant on raw fields, so both [`SandboxToml`] and `[verify]` share it: `locked`
/// REQUIRES network+proxy — so `compose_sandbox` is total and the "Locked ⇒ network+proxy" invariant is
/// structural (a typo'd/missing `network` can't yield a no-`--network` = full-internet container).
fn parse_egress_fields(
    egress: &str,
    network: &Option<String>,
    proxy: &Option<String>,
    no_proxy: &Option<String>,
) -> Result<bridge_core::domain::EgressPolicy, ConfigError> {
    use bridge_core::domain::EgressPolicy;
    match egress.to_ascii_lowercase().as_str() {
        "open" => Ok(EgressPolicy::Open),
        "locked" => {
            let network = network
                .clone()
                .ok_or_else(|| ConfigError::Registry("egress=locked requires network".into()))?;
            let proxy = proxy
                .clone()
                .ok_or_else(|| ConfigError::Registry("egress=locked requires proxy".into()))?;
            Ok(EgressPolicy::Locked {
                network,
                proxy,
                no_proxy: no_proxy.clone(),
            })
        }
        other => Err(ConfigError::Registry(format!(
            "invalid egress: {other:?} (expected locked|open)"
        ))),
    }
}

/// Convert the flat sandbox TOML egress into the data-carrying domain enum (delegates to the shared
/// field-level invariant).
fn parse_egress(t: &SandboxToml) -> Result<bridge_core::domain::EgressPolicy, ConfigError> {
    parse_egress_fields(&t.egress, &t.network, &t.proxy, &t.no_proxy)
}

impl RegistryConfig {
    /// Parse a multi-agent TOML string into a `RegistryConfig`.
    /// TOML deserialization plus env-expansion of the `[delegation]` `peer_url`/`auth`
    /// strings (so a `${PEER_TOKEN}`-style secret is resolved from the environment,
    /// matching the inbound-server expectation that the auth header is already concrete).
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        let mut cfg: RegistryConfig = toml::from_str(s)?;
        if let Some(d) = cfg.delegation.as_mut() {
            d.peer_url = expand_env(&d.peer_url)?;
            d.auth = expand_env(&d.auth)?;
        }
        if let Some(v) = &cfg.verify {
            if v.commands.is_some() {
                return Err(ConfigError::Registry(
                    "[verify].commands / [[verify.commands]] is removed — move them to [[languages.verify]]".into(),
                ));
            }
        }
        Ok(cfg)
    }

    pub fn language_profiles(
        &self,
    ) -> Result<Vec<bridge_core::profile::LanguageProfile>, ConfigError> {
        self.languages
            .iter()
            .map(LanguageToml::to_profile)
            .collect()
    }

    /// Parse each `[[workflows]]` entry: load prompt files from `base`, cross-check
    /// every `node.agent` against the declared `[[agents]]`, validate the DAG.
    /// Any failure is loud (`Err(ConfigError::Registry(...))`).
    pub fn load_workflows(
        &self,
        base: &std::path::Path,
    ) -> Result<
        std::collections::HashMap<
            bridge_core::ids::WorkflowId,
            std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
        >,
        ConfigError,
    > {
        use bridge_core::ids::{AgentId, NodeId, WorkflowId};
        use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};

        let agent_ids: std::collections::HashSet<&str> =
            self.agents.iter().map(|a| a.id.as_str()).collect();
        let mut map = std::collections::HashMap::new();
        for w in &self.workflows {
            let id = WorkflowId::parse(w.id.clone())
                .map_err(|e| ConfigError::Registry(format!("workflow id {:?}: {e:?}", w.id)))?;
            let mut nodes = Vec::with_capacity(w.nodes.len());
            for n in &w.nodes {
                if !agent_ids.contains(n.agent.as_str()) {
                    return Err(ConfigError::Registry(format!(
                        "workflow {} node {} references unknown agent {:?}",
                        w.id, n.id, n.agent
                    )));
                }
                let tpl = std::fs::read_to_string(base.join(&n.prompt_file)).map_err(|e| {
                    ConfigError::Registry(format!(
                        "workflow {} node {} prompt_file {:?}: {e}",
                        w.id, n.id, n.prompt_file
                    ))
                })?;
                nodes.push(WorkflowNode {
                    id: NodeId::parse(n.id.clone())
                        .map_err(|e| ConfigError::Registry(format!("node id {:?}: {e:?}", n.id)))?,
                    agent: AgentId::parse(n.agent.clone()).map_err(|e| {
                        ConfigError::Registry(format!("node agent {:?}: {e:?}", n.agent))
                    })?,
                    prompt_template: tpl,
                    inputs: n
                        .inputs
                        .iter()
                        .map(|i| NodeId::parse(i.clone()))
                        .collect::<Result<_, _>>()
                        .map_err(|e| {
                            ConfigError::Registry(format!("workflow {} input id: {e:?}", w.id))
                        })?,
                });
            }
            let g = WorkflowGraph {
                id: id.clone(),
                nodes,
            };
            g.validate()
                .map_err(|e| ConfigError::Registry(format!("workflow {} invalid: {e:?}", w.id)))?;
            map.insert(id, std::sync::Arc::new(g));
        }
        Ok(map)
    }

    /// Convert this parsed config into a `RegistrySnapshot` with typed domain values.
    pub fn into_snapshot(self) -> Result<RegistrySnapshot, ConfigError> {
        // The watchdog is PER-AGENT only; a top-level `[watchdog]` table would be a silent no-op
        // (every `AcpConfig.watchdog` stays `None`). Reject it so an operator isn't misled.
        if self.watchdog.is_some() {
            return Err(ConfigError::Registry(
                "top-level [watchdog] is not supported; configure it per agent as [agents.watchdog]"
                    .to_string(),
            ));
        }
        // The global cwd-gate root; captured before `self.agents` is moved by the loop below.
        let allowed_cwd_root = self.allowed_cwd_root.clone();
        // `allowed_cmds`: use the explicit list if provided; otherwise default to the union of all
        // entry cmds. S0 (dual-review): for a SANDBOXED entry the spawned program is the RUNTIME
        // (`sb.runtime()`), not `cmd` (the inner cli) — so default on the runtime, else the entry would
        // self-reject at the snapshot-layer S3 allowlist.
        let allowed_cmds =
            match self.registry {
                Some(r) if !r.allowed_cmds.is_empty() => r.allowed_cmds,
                _ => {
                    let mut v: Vec<String> =
                        self.agents
                            .iter()
                            .filter_map(|a| match &a.sandbox {
                                Some(sb) => Some(sb.runtime.clone().unwrap_or_else(|| {
                                    bridge_core::domain::DEFAULT_RUNTIME.into()
                                })),
                                None => a.cmd.clone(),
                            })
                            .collect();
                    v.sort();
                    v.dedup();
                    v
                }
            };

        let mut entries = Vec::with_capacity(self.agents.len());
        for a in self.agents {
            let id = AgentId::parse(a.id).map_err(|e| ConfigError::Registry(e.to_string()))?;
            let effort = a.effort.as_deref().map(parse_effort).transpose()?;
            let kind = match a.kind.as_deref() {
                Some(s) => parse_kind(s)?,
                None => AgentKind::default(),
            };
            // Parse-shape guard: per-kind cmd/base_url requirements. Placed before
            // `a.cmd`/`a.id` are moved into the constructed entry below.
            match kind {
                AgentKind::Acp if a.cmd.is_none() => {
                    return Err(ConfigError::Registry(format!(
                        "acp agent {:?} requires cmd",
                        id.as_str()
                    )));
                }
                AgentKind::Api if a.base_url.is_none() => {
                    return Err(ConfigError::Registry(format!(
                        "api agent {:?} requires base_url",
                        id.as_str()
                    )));
                }
                AgentKind::Api if a.cmd.is_some() => {
                    return Err(ConfigError::Registry(format!(
                        "api agent {:?} must not set cmd",
                        id.as_str()
                    )));
                }
                _ => {}
            }
            // Build the typed sandbox + S2 (mount == allowed_cwd_root). Stored NORMALIZED so the
            // snapshot-layer volume check (S6) compares like-for-like.
            // BOOT-FIXED (Codex): the live cwd gate reads `allowed_cwd_root` copied into InboundServer
            // ONCE at boot (main.rs ~1024); hot-reload re-applies only the RegistrySnapshot, not the
            // server root — so a sandbox mount/root change needs a RESTART. This S2 re-fires only where
            // `into_snapshot` runs (today the sole ConfigSource); a future 2nd source must re-thread it.
            let sandbox = match &a.sandbox {
                None => None,
                Some(sb) => {
                    let root = allowed_cwd_root.as_deref().ok_or_else(|| {
                        ConfigError::Registry(format!(
                            "sandboxed agent {:?} requires allowed_cwd_root",
                            id.as_str()
                        ))
                    })?;
                    let mount_n = bridge_core::SessionCwd::parse(&sb.mount)
                        .map_err(|e| ConfigError::Registry(format!("sandbox mount: {e:?}")))?;
                    let root_n = bridge_core::SessionCwd::parse(root)
                        .map_err(|e| ConfigError::Registry(format!("allowed_cwd_root: {e:?}")))?;
                    if mount_n.as_str() != root_n.as_str() {
                        return Err(ConfigError::Registry(format!(
                            "sandbox mount {:?} must equal allowed_cwd_root {:?}",
                            sb.mount, root
                        )));
                    }
                    Some(bridge_core::domain::SandboxConfig {
                        runtime: sb.runtime.clone(),
                        image: sb.image.clone(),
                        mount: mount_n.as_str().to_string(), // NORMALIZED
                        access: parse_access(&sb.access)?,
                        egress: parse_egress(sb)?,
                        volumes: sb.volumes.clone(),
                    })
                }
            };
            // MCP servers + delivery channel (ADR-0028). Validate {cwd} templating; resolve the
            // delivery from cmd basename (override via `mcp_delivery`).
            let mcp = build_mcp_specs(&a.mcp, id.as_str())?;
            let mcp_delivery = resolve_mcp_delivery(
                a.mcp_delivery.as_deref(),
                a.cmd.as_deref(),
                !mcp.is_empty(),
                id.as_str(),
            )?;
            // kiro MCP is host-only: the bridge writes ~/.kiro/agents/<name>.json on the HOST and points
            // kiro at it via `--agent`; a containerized kiro has its own home, so the config wouldn't reach
            // it (ADR-0028). Reject the combination rather than silently delivering nothing.
            if matches!(mcp_delivery, bridge_core::mcp::McpDelivery::KiroNative)
                && sandbox.is_some()
                && !mcp.is_empty()
            {
                return Err(ConfigError::Registry(format!(
                    "agent {:?}: kiro MCP delivery is host-only — remove [agents.sandbox] (kiro reads \
                     ~/.kiro/agents on the host), or use claude/codex for a containerized agent",
                    id.as_str()
                )));
            }
            let watchdog = match &a.watchdog {
                None => None,
                Some(wd) => {
                    if wd.idle_timeout_secs == 0 {
                        return Err(ConfigError::Registry(format!(
                            "agent {:?}: watchdog idle_timeout_secs must be > 0",
                            id.as_str()
                        )));
                    }
                    if wd.hard_wall_clock_secs == 0 {
                        return Err(ConfigError::Registry(format!(
                            "agent {:?}: watchdog hard_wall_clock_secs must be > 0",
                            id.as_str()
                        )));
                    }
                    // Cap at 30 days: a turn timeout above this is a config typo, and an
                    // unbounded value would overflow `Instant + Duration` and panic the watchdog
                    // task (the deadline math in acp_backend).
                    const WATCHDOG_MAX_SECS: u64 = 30 * 24 * 3600;
                    if wd.idle_timeout_secs > WATCHDOG_MAX_SECS
                        || wd.hard_wall_clock_secs > WATCHDOG_MAX_SECS
                    {
                        return Err(ConfigError::Registry(format!(
                            "agent {:?}: watchdog timeouts must be <= {WATCHDOG_MAX_SECS}s (30 days)",
                            id.as_str()
                        )));
                    }
                    Some(bridge_core::domain::WatchdogConfig {
                        idle_timeout: std::time::Duration::from_secs(wd.idle_timeout_secs),
                        hard_wall_clock: std::time::Duration::from_secs(wd.hard_wall_clock_secs),
                    })
                }
            };
            entries.push(AgentEntry {
                id,
                cmd: a.cmd,
                base_url: a.base_url,
                api_key_env: a.api_key_env,
                args: a.args,
                kind,
                model_provider: a.model_provider,
                model: a.model,
                effort,
                mode: a.mode,
                cwd: a.cwd,
                session_cwd: a.session_cwd,
                sandbox,
                watchdog,
                mcp,
                mcp_delivery,
                auth_method: a.auth_method,
                name: a.name,
                description: a.description,
                tags: a.tags,
                version: a.version,
                extensions: a.extensions,
            });
        }

        let default =
            AgentId::parse(self.default).map_err(|e| ConfigError::Registry(e.to_string()))?;

        Ok(RegistrySnapshot {
            default,
            entries,
            allowed_cmds,
        })
    }
}

impl LanguageToml {
    pub fn to_profile(&self) -> Result<bridge_core::profile::LanguageProfile, ConfigError> {
        if self.verify.is_empty() {
            return Err(ConfigError::Registry(format!(
                "[[languages]] id={:?} needs at least one [[languages.verify]] command",
                self.id
            )));
        }
        Ok(bridge_core::profile::LanguageProfile::from_parts(
            self.id.clone(),
            self.fetch.clone(),
            self.warm_cache.clone(),
            self.dep_cache_path.clone(),
            self.verify_cache_path.clone(),
            map_pairs(&self.fetch_env),
            map_pairs(&self.lsp_env),
            map_pairs(&self.verify_env),
            self.image.clone(),
            self.verify
                .iter()
                .map(|v| bridge_core::profile::VerifyCommand {
                    name: v.name.clone(),
                    cmd: v.cmd.clone(),
                    gate: v.gate,
                })
                .collect(),
        ))
    }
}

fn map_pairs(m: &Option<std::collections::BTreeMap<String, String>>) -> Vec<(String, String)> {
    m.as_ref()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// Parse an effort-level string into the `Effort` enum.
/// Valid inputs: "minimal", "low", "medium", "high", "xhigh", "max" (case-insensitive).
fn parse_effort(s: &str) -> Result<Effort, ConfigError> {
    s.parse::<Effort>().map_err(ConfigError::Registry)
}

/// Parse the adapter-kind string into `AgentKind`. None → Acp (back-compat).
fn parse_kind(s: &str) -> Result<AgentKind, ConfigError> {
    Ok(match s {
        "acp" => AgentKind::Acp,
        "api" => AgentKind::Api,
        "container_rw" => AgentKind::ContainerRw,
        other => {
            return Err(ConfigError::Registry(format!(
                "invalid kind: {other:?} (expected acp|api|container_rw)"
            )));
        }
    })
}

// ---------------------------------------------------------------------------
// FileConfigSource — the File `ConfigSource` adapter (Task 8 / Increment 3b).
//
// `load()` reads + parses the TOML at `path` into a `RegistrySnapshot` (via the
// Task-7 `RegistryConfig::parse` → `into_snapshot` pipeline). `watch()` returns a
// stream that fires whenever the file changes on disk.
//
// The four must-haves for a robust file watch:
//   (a) PARENT-DIR watch — editors save by atomic-rename (write `.tmp`, rename over
//       the target), which gives the file a NEW inode; a file-inode watch goes stale
//       and silently misses the edit. Watching the parent directory survives this.
//   (b) DEBOUNCE — one logical save can emit several fs events; we coalesce a burst
//       into a single re-load with a short settle window.
//   (c) WATCHER KEPT ALIVE — `notify::RecommendedWatcher` stops delivering events the
//       moment it is dropped, so it MUST be moved into (and live for the whole life of)
//       the spawned task.
//   (d) KEEP-LAST-GOOD — a transient parse failure (e.g. a half-written file) MUST NOT
//       tear the stream down; we log and skip emitting, leaving the consumer on the
//       last good snapshot.
// ---------------------------------------------------------------------------

/// File-backed [`ConfigSource`](bridge_core::ports::ConfigSource): loads a
/// `RegistrySnapshot` from a TOML file and watches its parent directory for edits.
pub struct FileConfigSource {
    path: std::path::PathBuf,
}

impl FileConfigSource {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Read + parse the TOML at `path` into a `RegistrySnapshot`. Shared by `load()`
    /// and the watch task's re-load. `None` on read/parse failure (so the watch task
    /// can keep-last-good); `load()` maps the failure to a `BridgeError` instead.
    async fn try_load(path: &std::path::Path) -> Option<RegistrySnapshot> {
        let s = tokio::fs::read_to_string(path).await.ok()?;
        RegistryConfig::parse(&s)
            .and_then(|c| c.into_snapshot())
            .ok()
    }
}

#[async_trait::async_trait]
impl bridge_core::ports::ConfigSource for FileConfigSource {
    async fn load(&self) -> Result<RegistrySnapshot, bridge_core::error::BridgeError> {
        let s = tokio::fs::read_to_string(&self.path).await.map_err(|e| {
            bridge_core::error::BridgeError::ConfigInvalid {
                reason: format!("read {}: {e}", self.path.display()),
            }
        })?;
        RegistryConfig::parse(&s)
            .and_then(|c| c.into_snapshot())
            .map_err(|e| bridge_core::error::BridgeError::ConfigInvalid {
                reason: e.to_string(),
            })
    }

    fn watch(&self) -> futures::stream::BoxStream<'static, RegistrySnapshot> {
        let path = self.path.clone();
        // (a) Watch the PARENT directory, not the file inode — atomic-rename saves
        // replace the inode, which a file-watch would miss after the first event.
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        // The filename we re-load on any relevant directory event.
        let file_name = path.file_name().map(|n| n.to_os_string());

        let (tx, rx) = tokio::sync::mpsc::channel::<RegistrySnapshot>(8);

        // notify's callback runs on its own thread; bridge its events to async land
        // over an unbounded channel of "something changed" signals.
        let (raw_tx, mut raw_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let filter_name = file_name.clone();
        // Create + REGISTER the watcher SYNCHRONOUSLY, before this function returns.
        // Registering inside the spawned task would race the caller: a `watch()`-then-
        // edit sequence could fire the (single) edit before the watcher is live and miss
        // it forever. Events that land before the loop starts are buffered in `raw_rx`.
        let watcher =
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res {
                    // Filter to events touching OUR file (by filename) when notify gives us
                    // paths; if it gives none, treat it as a coarse signal and re-check by
                    // path below. Robust to atomic-rename, which reports the target path in
                    // the rename's `paths`.
                    let relevant = match &filter_name {
                        Some(name) => {
                            ev.paths.is_empty()
                                || ev
                                    .paths
                                    .iter()
                                    .any(|p| p.file_name() == Some(name.as_os_str()))
                        }
                        None => true,
                    };
                    if relevant {
                        let _ = raw_tx.send(());
                    }
                }
            }) {
                Ok(w) => Some(w),
                Err(e) => {
                    tracing::warn!(error = %e, "config watcher init failed; watch disabled");
                    None
                }
            };
        let watcher = watcher.and_then(|mut w| {
            use notify::Watcher;
            match w.watch(&parent, notify::RecursiveMode::NonRecursive) {
                Ok(()) => Some(w),
                Err(e) => {
                    tracing::warn!(dir = %parent.display(), error = %e, "config watch failed; watch disabled");
                    None
                }
            }
        });

        tokio::spawn(async move {
            // (c) Keep the watcher alive for the whole task — `notify` stops delivering
            // events the instant it is dropped. `None` = init failed; the loop below then
            // idles until the receiver is dropped.
            let _watcher = watcher;

            loop {
                // Block until at least one change signal arrives.
                if raw_rx.recv().await.is_none() {
                    break; // watcher dropped (only happens at task end) → stop.
                }
                // (b) Debounce: let a burst of events for one logical save settle, then
                // drain the backlog so we re-load exactly once per settled edit.
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                while raw_rx.try_recv().is_ok() {}

                // Re-load by PATH (not inode) so we pick up the freshly-renamed file.
                match Self::try_load(&path).await {
                    // (d) Keep-last-good: only emit on a successful parse.
                    Some(snap) => {
                        if tx.send(snap).await.is_err() {
                            break; // (e) receiver dropped → stop the task.
                        }
                    }
                    None => {
                        tracing::warn!(
                            path = %path.display(),
                            "config reload failed; keeping last-good"
                        );
                    }
                }
            }

            // (c) `_watcher` lived for the whole task; drop it explicitly here.
            drop(_watcher);
        });

        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vc(runtime: Option<&str>) -> VerifyConfig {
        VerifyConfig {
            runtime: runtime.map(str::to_string),
            image: "img".into(),
            cache: "c".into(),
            egress: bridge_core::domain::EgressPolicy::Open,
        }
    }

    #[test]
    fn gate_rejects_defaulted_runtime_when_only_podman_allowed() {
        let out = gate_verify_runtime(Some(Ok(vc(None))), &["podman".to_string()]);
        let err = out.unwrap().unwrap_err();
        assert!(
            format!("{err:?}").contains("docker"),
            "rejection names the resolved default 'docker'"
        );
    }

    #[test]
    fn gate_rejects_explicit_disallowed_runtime() {
        let out = gate_verify_runtime(Some(Ok(vc(Some("docker")))), &["podman".to_string()]);
        assert!(out.unwrap().is_err());
    }

    #[test]
    fn gate_allows_explicit_allowed_runtime() {
        let out = gate_verify_runtime(Some(Ok(vc(Some("podman")))), &["podman".to_string()]);
        assert_eq!(out.unwrap().unwrap().runtime.as_deref(), Some("podman"));
    }

    #[test]
    fn gate_back_compat_defaulted_docker_allowed_and_normalized() {
        let out = gate_verify_runtime(
            Some(Ok(vc(None))),
            &["docker".to_string(), "codex-acp".to_string()],
        );
        // Existing docker configs unaffected, AND the defaulted runtime is normalized to an explicit value
        // so no downstream consumer re-defaults (review MAJOR 4).
        assert_eq!(out.unwrap().unwrap().runtime.as_deref(), Some("docker"));
    }

    #[test]
    fn gate_preserves_prior_error() {
        let prior = Err(ConfigError::Registry("prior config error".into()));
        let out = gate_verify_runtime(Some(prior), &["podman".to_string()]);
        assert!(
            matches!(out, Some(Err(ConfigError::Registry(m))) if m.contains("prior config error"))
        );
    }

    #[test]
    fn gate_passes_through_none() {
        assert!(gate_verify_runtime(None, &["podman".to_string()]).is_none());
    }

    #[test]
    fn delegation_parsed_with_env_expansion() {
        std::env::set_var("PEER_TOKEN_T10", "sek");
        let c = RegistryConfig::parse(
            "default=\"k\"\n[[agents]]\nid=\"k\"\ncmd=\"k\"\nargs=[]\n[server]\n\
             [delegation]\npeer_url=\"http://p\"\nauth=\"bearer:${PEER_TOKEN_T10}\"\n",
        )
        .unwrap();
        let d = c.delegation.unwrap();
        assert_eq!(d.peer_url, "http://p");
        assert_eq!(d.auth, "bearer:sek");
    }

    #[test]
    fn config_without_delegation_still_valid() {
        let c = RegistryConfig::parse(
            "default=\"k\"\n[[agents]]\nid=\"k\"\ncmd=\"k\"\nargs=[]\n[server]\n",
        )
        .unwrap();
        assert!(c.delegation.is_none());
        assert_eq!(c.server.addr, "127.0.0.1:8080");
    }

    #[test]
    fn missing_env_var_errors() {
        let r = RegistryConfig::parse(
            "default=\"k\"\n[[agents]]\nid=\"k\"\ncmd=\"k\"\nargs=[]\n[server]\n\
             [delegation]\npeer_url=\"http://p\"\nauth=\"bearer:${DEFINITELY_UNSET_VAR_XYZ}\"\n",
        );
        assert!(matches!(r, Err(ConfigError::MissingEnvVar(_))));
    }

    // -----------------------------------------------------------------------
    // RegistryConfig / RegistrySnapshot tests (Task 7 / Increment 3b)
    // -----------------------------------------------------------------------

    #[test]
    fn parses_agents_and_default() {
        let toml = r#"
default = "codex"

[registry]
allowed_cmds = ["codex-acp", "kiro-cli"]

[[agents]]
id = "codex"
cmd = "codex-acp"
model = "gpt-5.5"
effort = "high"
mode = "read-only"

[[agents]]
id = "kiro"
cmd = "kiro-cli"
args = ["acp"]

[server]
addr = "127.0.0.1:8080"
"#;
        let snap = RegistryConfig::parse(toml)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.default.as_str(), "codex");
        assert_eq!(snap.entries.len(), 2);
        assert!(snap.allowed_cmds.contains(&"kiro-cli".to_string()));
        let codex = snap
            .entries
            .iter()
            .find(|e| e.id.as_str() == "codex")
            .unwrap();
        assert_eq!(codex.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(codex.effort, Some(bridge_core::domain::Effort::High));
        let kiro = snap
            .entries
            .iter()
            .find(|e| e.id.as_str() == "kiro")
            .unwrap();
        assert_eq!(kiro.args, vec!["acp".to_string()]);
    }

    #[test]
    fn allowed_cmds_defaults_to_entry_cmds_when_absent() {
        // A TOML with NO [registry] section → allowed_cmds defaults to the set of entry cmds.
        let toml = r#"
default = "alpha"

[[agents]]
id = "alpha"
cmd = "alpha-cli"

[[agents]]
id = "beta"
cmd = "beta-cli"

[server]
"#;
        let snap = RegistryConfig::parse(toml)
            .unwrap()
            .into_snapshot()
            .unwrap();
        // Both cmds should be in allowed_cmds (sorted + deduped).
        assert!(snap.allowed_cmds.contains(&"alpha-cli".to_string()));
        assert!(snap.allowed_cmds.contains(&"beta-cli".to_string()));
        assert_eq!(snap.allowed_cmds.len(), 2);
    }

    #[test]
    fn effort_parses_all_levels_and_rejects_invalid() {
        // All valid levels round-trip.
        for (s, expected) in [
            ("minimal", bridge_core::domain::Effort::Minimal),
            ("low", bridge_core::domain::Effort::Low),
            ("medium", bridge_core::domain::Effort::Medium),
            ("high", bridge_core::domain::Effort::High),
            ("xhigh", bridge_core::domain::Effort::Xhigh),
            ("max", bridge_core::domain::Effort::Max),
        ] {
            assert_eq!(parse_effort(s).unwrap(), expected, "failed for {s:?}");
        }
        // Invalid value → Err(ConfigError::Registry).
        let err = parse_effort("bogus").unwrap_err();
        assert!(
            matches!(err, ConfigError::Registry(_)),
            "expected Registry variant, got: {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // FileConfigSource tests (Task 8 / Increment 3b)
    // -----------------------------------------------------------------------

    const V1_STRING: &str = r#"default="codex"
[registry]
allowed_cmds=["codex-acp"]
[[agents]]
id="codex"
cmd="codex-acp"
[server]
addr="127.0.0.1:8080"
"#;

    // A self-consistent v2: default="kiro", one agent id="kiro"/cmd="kiro-cli",
    // allowed_cmds=["kiro-cli"].
    const V2_STRING: &str = r#"default="kiro"
[registry]
allowed_cmds=["kiro-cli"]
[[agents]]
id="kiro"
cmd="kiro-cli"
args=["acp"]
[server]
addr="127.0.0.1:8080"
"#;

    #[tokio::test]
    async fn load_parses_via_into_snapshot() {
        use bridge_core::ports::ConfigSource;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a-bridge.toml");
        std::fs::write(&path, V1_STRING).unwrap();
        let src = FileConfigSource::new(path.clone());
        let snap = src.load().await.unwrap();
        assert_eq!(snap.default.as_str(), "codex");
        assert_eq!(snap.entries.len(), 1);
    }

    #[tokio::test]
    async fn load_errors_on_missing_file() {
        use bridge_core::ports::ConfigSource;
        let dir = tempfile::tempdir().unwrap();
        let src = FileConfigSource::new(dir.path().join("does-not-exist.toml"));
        let err = src.load().await.unwrap_err();
        assert!(
            matches!(err, bridge_core::error::BridgeError::ConfigInvalid { .. }),
            "expected ConfigInvalid, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn watch_emits_on_edit_via_atomic_rename() {
        use bridge_core::ports::ConfigSource;
        use futures::StreamExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a-bridge.toml");
        std::fs::write(&path, V1_STRING).unwrap();

        let src = FileConfigSource::new(path.clone());
        // load() returns v1.
        assert_eq!(src.load().await.unwrap().default.as_str(), "codex");

        // Start watching, then ATOMICALLY RENAME a v2 over the file (editor-style
        // save → new inode — the footgun a file-inode watch silently misses).
        let mut stream = src.watch();
        let tmp = dir.path().join(".a2a-bridge.toml.tmp");
        std::fs::write(&tmp, V2_STRING).unwrap();
        std::fs::rename(&tmp, &path).unwrap();

        // A snapshot with default "kiro" must arrive within the timeout. The window
        // is generous (200ms debounce + fs-event latency) to stay non-flaky.
        let snap = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("watch must emit within 5s")
            .expect("stream not ended");
        assert_eq!(snap.default.as_str(), "kiro");
    }

    #[tokio::test]
    async fn watch_keeps_last_good_on_parse_error() {
        use bridge_core::ports::ConfigSource;
        use futures::StreamExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a-bridge.toml");
        std::fs::write(&path, V1_STRING).unwrap();

        let src = FileConfigSource::new(path.clone());
        let mut stream = src.watch();

        // First write GARBAGE (parse fails) — must NOT emit, must NOT tear down.
        let tmp = dir.path().join(".garbage.tmp");
        std::fs::write(&tmp, "this is not valid toml = = =").unwrap();
        std::fs::rename(&tmp, &path).unwrap();

        // Then write a valid v2 — the stream survives and emits the good snapshot.
        let tmp2 = dir.path().join(".v2.tmp");
        std::fs::write(&tmp2, V2_STRING).unwrap();
        std::fs::rename(&tmp2, &path).unwrap();

        let snap = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("watch must still emit after a transient parse error")
            .expect("stream not ended");
        assert_eq!(snap.default.as_str(), "kiro");
    }

    // -----------------------------------------------------------------------
    // Task 13: kind parse + warm-pool extension getters
    // -----------------------------------------------------------------------

    #[test]
    fn kind_parses_and_defaults_to_acp() {
        let snap = RegistryConfig::parse(
            "default=\"c\"\n[[agents]]\nid=\"c\"\ncmd=\"codex-acp\"\nkind=\"acp\"\n\
             [[agents]]\nid=\"k\"\ncmd=\"kiro-cli\"\n[server]\n",
        )
        .unwrap()
        .into_snapshot()
        .unwrap();
        let c = snap.entries.iter().find(|e| e.id.as_str() == "c").unwrap();
        let k = snap.entries.iter().find(|e| e.id.as_str() == "k").unwrap();
        assert_eq!(c.kind, bridge_core::domain::AgentKind::Acp); // explicit
        assert_eq!(k.kind, bridge_core::domain::AgentKind::Acp); // default
    }

    #[test]
    fn invalid_kind_is_config_error() {
        let r = RegistryConfig::parse(
            "default=\"c\"\n[[agents]]\nid=\"c\"\ncmd=\"claude\"\nkind=\"nope\"\n[server]\n",
        )
        .unwrap()
        .into_snapshot();
        assert!(r.is_err());
    }

    // -----------------------------------------------------------------------
    // Task 15: surface-A ripple — kind="api", cmd optional, base_url
    // -----------------------------------------------------------------------

    #[test]
    fn parse_kind_accepts_api() {
        assert_eq!(
            parse_kind("api").unwrap(),
            bridge_core::domain::AgentKind::Api
        );
        assert!(parse_kind("bogus").is_err());
    }

    // --- B1 [sandbox] parse layer (S0 / S2 / EgressPolicy conversion) ----------

    const SB_OK: &str = "default=\"a\"\nallowed_cwd_root=\"/work\"\n[[agents]]\nid=\"a\"\ncmd=\"claude-agent-acp\"\n[agents.sandbox]\nimage=\"img\"\nmount=\"/work\"\naccess=\"ro\"\negress=\"open\"\n[server]\n";

    #[test]
    fn sandbox_mount_must_equal_allowed_cwd_root() {
        assert!(RegistryConfig::parse(SB_OK)
            .unwrap()
            .into_snapshot()
            .is_ok());
        let bad = SB_OK.replace("mount=\"/work\"", "mount=\"/work/sub\"");
        assert!(
            RegistryConfig::parse(&bad)
                .unwrap()
                .into_snapshot()
                .is_err(),
            "mount != allowed_cwd_root must reject"
        );
    }

    #[test]
    fn sandbox_default_allowed_cmds_uses_runtime_not_cli() {
        // No [registry] → allowed_cmds defaults; a sandboxed agent must NOT self-reject (default docker).
        let snap = RegistryConfig::parse(SB_OK)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert!(snap.allowed_cmds.contains(&"docker".to_string()));
        assert!(!snap.allowed_cmds.contains(&"claude-agent-acp".to_string()));
    }

    #[test]
    fn sandbox_egress_locked_requires_network_and_proxy() {
        let bad = SB_OK.replace("egress=\"open\"", "egress=\"locked\"");
        assert!(
            RegistryConfig::parse(&bad)
                .unwrap()
                .into_snapshot()
                .is_err(),
            "locked without network/proxy must reject at the EgressPolicy conversion"
        );
    }

    #[test]
    fn sandbox_requires_allowed_cwd_root() {
        // S2: a sandboxed entry with NO allowed_cwd_root must fail into_snapshot.
        let bad = SB_OK.replace("allowed_cwd_root=\"/work\"\n", "");
        assert!(RegistryConfig::parse(&bad)
            .unwrap()
            .into_snapshot()
            .is_err());
    }

    #[test]
    fn watchdog_toml_parses_per_agent() {
        let toml = r#"
            default = "c"
            [server]
            addr = "127.0.0.1:8080"
            [[agents]]
            id = "c"
            cmd = "codex-acp"
            [agents.watchdog]
            idle_timeout_secs = 30
            hard_wall_clock_secs = 600
            [[agents]]
            id = "plain"
            cmd = "codex-acp"
        "#;
        let snap = RegistryConfig::parse(toml)
            .unwrap()
            .into_snapshot()
            .unwrap();
        let wd = snap
            .entries
            .iter()
            .find(|e| e.id.as_str() == "c")
            .unwrap()
            .watchdog
            .as_ref()
            .unwrap();
        assert_eq!(wd.idle_timeout, std::time::Duration::from_secs(30));
        assert_eq!(wd.hard_wall_clock, std::time::Duration::from_secs(600));
        assert!(
            snap.entries
                .iter()
                .find(|e| e.id.as_str() == "plain")
                .unwrap()
                .watchdog
                .is_none(),
            "agents without [agents.watchdog] must leave watchdog disabled"
        );

        let bad = toml.replace("idle_timeout_secs = 30", "idle_timeout_secs = 0");
        assert!(
            RegistryConfig::parse(&bad)
                .unwrap()
                .into_snapshot()
                .is_err(),
            "idle_timeout_secs = 0 must be rejected"
        );
        let bad = toml.replace("hard_wall_clock_secs = 600", "hard_wall_clock_secs = 0");
        assert!(
            RegistryConfig::parse(&bad)
                .unwrap()
                .into_snapshot()
                .is_err(),
            "hard_wall_clock_secs = 0 must be rejected"
        );
        // > 30 days is rejected (an unbounded value would overflow Instant+Duration).
        let bad = toml.replace(
            "hard_wall_clock_secs = 600",
            "hard_wall_clock_secs = 9999999999",
        );
        assert!(
            RegistryConfig::parse(&bad)
                .unwrap()
                .into_snapshot()
                .is_err(),
            "an absurd hard_wall_clock_secs must be rejected (overflow guard)"
        );
    }

    #[test]
    fn top_level_watchdog_table_is_rejected() {
        // The watchdog is PER-AGENT; a top-level [watchdog] would be a silent no-op -> reject it.
        let toml = r#"
            default = "c"
            [server]
            addr = "127.0.0.1:8080"
            [watchdog]
            idle_timeout_secs = 30
            hard_wall_clock_secs = 600
            [[agents]]
            id = "c"
            cmd = "codex-acp"
        "#;
        assert!(
            RegistryConfig::parse(toml)
                .unwrap()
                .into_snapshot()
                .is_err(),
            "a top-level [watchdog] table must be rejected (use [agents.watchdog])"
        );
    }

    #[test]
    fn verify_config_parses_structured_commands_and_locked_egress() {
        let c = RegistryConfig::parse(
            r#"
            default = "x"
            [server]
            addr = "127.0.0.1:8080"
            [[agents]]
            id = "x"
            cmd = "echo"
            [verify]
            image = "a2a-toolchain:latest"
            cache = "a2a-verify-cache"
            egress = "locked"
            network = "a2a-verify-egress"
            proxy = "http://a2a-verify-proxy:8888"
            [[languages]]
            id = "rust"
            fetch = "cargo fetch --locked"
            warm_cache = "a2a-impl-lsp-cache"
            dep_cache_path = "/cargo"
            verify_cache_path = "/cache"
            [[languages.verify]]
            name = "fmt"
            cmd = "cargo fmt --all -- --check"
            [[languages.verify]]
            name = "test"
            cmd = "cargo test --locked"
            gate = false
            "#,
        )
        .unwrap();
        let v = c.verify.as_ref().unwrap().to_config().unwrap();
        assert_eq!(v.image, "a2a-toolchain:latest");
        assert_eq!(v.cache, "a2a-verify-cache");
        assert!(matches!(
            v.egress,
            bridge_core::domain::EgressPolicy::Locked { .. }
        ));
        let profiles = c.language_profiles().unwrap();
        assert_eq!(profiles.len(), 1);
        let commands = &profiles[0].verify_commands;
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].name, "fmt");
        assert!(commands[0].gate); // gate defaults to true
        assert!(!commands[1].gate); // explicit gate=false
    }

    #[test]
    fn verify_config_rejects_locked_without_network() {
        let c = RegistryConfig::parse(
            r#"
            default = "x"
            [server]
            addr = "127.0.0.1:8080"
            [[agents]]
            id = "x"
            cmd = "echo"
            [verify]
            image = "i"
            cache = "c"
            egress = "locked"
            proxy = "http://p:8888"
            "#,
        )
        .unwrap();
        let e = c.verify.as_ref().unwrap().to_config().unwrap_err();
        assert!(format!("{e:?}").contains("requires network"));
    }

    #[test]
    fn legacy_verify_commands_is_rejected() {
        let e = RegistryConfig::parse(
            r#"
            default = "x"
            [server]
            addr = "127.0.0.1:8080"
            [[agents]]
            id = "x"
            cmd = "echo"
            [verify]
            image = "i"
            cache = "c"
            egress = "open"
            [[verify.commands]]
            name = "t"
            cmd = "cargo test"
            "#,
        )
        .unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("verify.commands"));
        assert!(msg.contains("languages"));
    }

    #[test]
    fn profile_needs_at_least_one_verify_command() {
        let c = RegistryConfig::parse(
            r#"
            default = "x"
            [server]
            addr = "127.0.0.1:8080"
            [[agents]]
            id = "x"
            cmd = "echo"
            [[languages]]
            id = "rust"
            fetch = "cargo fetch --locked"
            warm_cache = "a2a-impl-lsp-cache"
            dep_cache_path = "/cargo"
            verify_cache_path = "/cache"
            "#,
        )
        .unwrap();
        let e = c.language_profiles().unwrap_err();
        assert!(e.to_string().contains("at least one"));
    }

    #[test]
    fn tracked_example_language_verify_commands_round_trip() {
        // Pin the FULL (name, cmd, gate) per config — not just names — so a changed test flag, a dropped
        // `--exclude`/`--skip`, or clippy re-added to slicing is caught (gpt-5.5 review MINOR).
        let fmt = ("fmt", "cargo fmt --all -- --check", true);
        let clippy = (
            "clippy",
            "cargo clippy --all-targets --all-features --locked -- -D warnings",
            true,
        );
        let build = ("build", "cargo build --locked", true);
        let bridge_test = (
            "test",
            "cargo test --workspace --locked --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill --skip process::tests::drop_group_kills_descendants",
            true,
        );
        let slicing_test = ("test", "cargo test --locked", true);
        let cases = [
            (
                include_str!("../../../examples/a2a-bridge.containerized.toml"),
                vec![fmt, clippy, build, bridge_test],
            ),
            (
                include_str!("../../../examples/a2a-bridge.containerized.podman.toml"),
                vec![fmt, clippy, build, bridge_test],
            ),
            (
                include_str!("../../../examples/a2a-bridge.slicing-implement.toml"),
                vec![fmt, build, slicing_test],
            ),
        ];
        for (raw, expected) in cases {
            let profiles = RegistryConfig::parse(raw)
                .unwrap()
                .language_profiles()
                .unwrap();
            let rust = profiles.iter().find(|p| p.id == "rust").unwrap();
            let got: Vec<(&str, &str, bool)> = rust
                .verify_commands
                .iter()
                .map(|c| (c.name.as_str(), c.cmd.as_str(), c.gate))
                .collect();
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn example_containerized_go_language_profile() {
        // Check both configs symmetrically — the go profile must be identical in each.
        for raw in [
            include_str!("../../../examples/a2a-bridge.containerized.toml"),
            include_str!("../../../examples/a2a-bridge.containerized.podman.toml"),
        ] {
            let cfg = RegistryConfig::parse(raw).unwrap();
            let profiles = cfg.language_profiles().unwrap();
            let go = profiles
                .iter()
                .find(|p| p.id == "go")
                .expect("go profile present in containerized config");
            // Pin full (name, cmd, gate) tuples — same standard as tracked_example_language_verify_commands_round_trip.
            let got: Vec<(&str, &str, bool)> = go
                .verify_commands
                .iter()
                .map(|c| (c.name.as_str(), c.cmd.as_str(), c.gate))
                .collect();
            assert_eq!(
                got,
                vec![
                    ("gofmt", "test -z \"$(gofmt -l .)\"", true),
                    ("vet", "go vet ./...", true),
                    ("build", "go build ./...", true),
                    ("test", "go test ./...", true),
                ]
            );
        }
    }

    #[test]
    fn example_containerized_python_language_profile() {
        // Check both configs symmetrically — the python profile must be identical in each (guards Docker/
        // Podman drift + the load-bearing uv/pyvenv command strings; the live gate validated them in-container).
        for raw in [
            include_str!("../../../examples/a2a-bridge.containerized.toml"),
            include_str!("../../../examples/a2a-bridge.containerized.podman.toml"),
        ] {
            let cfg = RegistryConfig::parse(raw).unwrap();
            let profiles = cfg.language_profiles().unwrap();
            let py = profiles
                .iter()
                .find(|p| p.id == "python")
                .expect("python profile present in containerized config");
            // Python's "dep cache" is the venv at /pyvenv; fetch ALWAYS creates it then best-effort installs
            // deps (uv sync honors UV_PROJECT_ENVIRONMENT; the requirements fallback honors VIRTUAL_ENV).
            assert!(
                py.fetch_cmd.contains("uv venv /pyvenv")
                    && py
                        .fetch_cmd
                        .contains("UV_PROJECT_ENVIRONMENT=/pyvenv uv sync --frozen")
                    && py
                        .fetch_cmd
                        .contains("VIRTUAL_ENV=/pyvenv uv pip install -r requirements.txt"),
                "python fetch must always-create /pyvenv + dual-path deps: {}",
                py.fetch_cmd
            );
            // The #1d keystone: basedpyright's interpreter override reaches the LSP via the Lsp cache binding.
            let lsp_env = py
                .cache_binding(bridge_core::profile::CacheCtx::Lsp, "", "")
                .env;
            assert!(
                lsp_env
                    .iter()
                    .any(|(k, v)| k == "LSP_MCP_PYTHON_PATH" && v == "/pyvenv/bin/python"),
                "python lsp_env must point basedpyright at the venv interpreter: {lsp_env:?}"
            );
            // Pin full (name, cmd, gate) verify tuples — same standard as the go profile test.
            let got: Vec<(&str, &str, bool)> = py
                .verify_commands
                .iter()
                .map(|c| (c.name.as_str(), c.cmd.as_str(), c.gate))
                .collect();
            assert_eq!(
                got,
                vec![
                    ("format", "ruff format --check .", true),
                    ("lint", "ruff check .", true),
                    (
                        "test",
                        "uv venv /cache/venv && { UV_PROJECT_ENVIRONMENT=/cache/venv uv sync --frozen || VIRTUAL_ENV=/cache/venv uv pip install -r requirements.txt; } && /cache/venv/bin/python -m pytest -q",
                        true
                    ),
                ]
            );
        }
    }

    #[test]
    fn example_containerized_typescript_language_profile() {
        // Check both configs symmetrically — the typescript profile must be identical in each (guards Docker/
        // Podman drift + the load-bearing npm/tsc command strings; validated by the Task 3 live gate).
        for raw in [
            include_str!("../../../examples/a2a-bridge.containerized.toml"),
            include_str!("../../../examples/a2a-bridge.containerized.podman.toml"),
        ] {
            let cfg = RegistryConfig::parse(raw).unwrap();
            let profiles = cfg.language_profiles().unwrap();
            let ts = profiles
                .iter()
                .find(|p| p.id == "typescript")
                .expect("typescript profile present in containerized config");
            // Fetch: copy manifests to / then `cd /` + `npm ci` (installs into /node_modules root-walk
            // target, spiked). The fetch command copies the manifests into / so `npm ci` runs at / and
            // populates the mounted /node_modules volume.
            assert!(
                ts.fetch_cmd.contains("cp /work/package.json /")
                    && ts.fetch_cmd.contains("cp /work/package-lock.json /")
                    && ts.fetch_cmd.contains("npm ci --ignore-scripts"),
                "typescript fetch must copy manifests (separately/tolerant) to / then npm ci --ignore-scripts: {}",
                ts.fetch_cmd
            );
            // lsp_env is empty for TS (swappable via LSP_MCP_TS_SERVER in lsp_env; default unset → tsls).
            let lsp_env = ts
                .cache_binding(bridge_core::profile::CacheCtx::Lsp, "", "")
                .env;
            assert!(
                lsp_env.is_empty(),
                "typescript lsp_env must be empty (swap via lsp_env override, not a default): {lsp_env:?}"
            );
            // Pin full (name, cmd, gate) verify tuples.
            let got: Vec<(&str, &str, bool)> = ts
                .verify_commands
                .iter()
                .map(|c| (c.name.as_str(), c.cmd.as_str(), c.gate))
                .collect();
            assert_eq!(
                got,
                vec![
                    (
                        "typecheck",
                        "cp package.json / 2>/dev/null; cp package-lock.json / 2>/dev/null; (cd / && { npm ci --ignore-scripts || npm install --ignore-scripts || true; }); tsc --noEmit",
                        true
                    ),
                    (
                        "lint",
                        "if ls .eslintrc* eslint.config.* >/dev/null 2>&1; then PATH=/node_modules/.bin:$PATH eslint .; else echo 'no eslint config; skip'; fi",
                        true
                    ),
                    ("test", "npm test --if-present", true),
                ]
            );
        }
    }

    #[test]
    fn example_containerized_impl_lsp_lang_is_auto() {
        // Check both configs symmetrically.
        for raw in [
            include_str!("../../../examples/a2a-bridge.containerized.toml"),
            include_str!("../../../examples/a2a-bridge.containerized.podman.toml"),
        ] {
            let cfg = RegistryConfig::parse(raw).unwrap();
            let impl_agent = cfg
                .agents
                .iter()
                .find(|a| a.id == "impl")
                .expect("impl agent present");
            let lsp = impl_agent
                .mcp
                .iter()
                .find(|m| m.name == "lsp")
                .expect("lsp mcp server present");
            let lang_pos = lsp
                .args
                .iter()
                .position(|a| a == "--lang")
                .expect("--lang arg present");
            assert_eq!(lsp.args[lang_pos + 1], "auto", "--lang value must be auto");
            // M2 wiring landed: CARGO_HOME + CARGO_NET_OFFLINE come from the rust profile's
            // lsp_env (injected at runtime by apply_lsp_env) -- they must NOT be in the static config.
            assert!(
                !lsp.env.iter().any(|e| e.name == "CARGO_HOME"),
                "CARGO_HOME must not be in impl lsp config env (now injected from profile)"
            );
            assert!(
                !lsp.env.iter().any(|e| e.name == "CARGO_NET_OFFLINE"),
                "CARGO_NET_OFFLINE must not be in impl lsp config env (now injected from profile)"
            );
            // LSP_MCP_LOG must remain in the static config (not profile-owned).
            assert!(
                lsp.env.iter().any(|e| e.name == "LSP_MCP_LOG"),
                "LSP_MCP_LOG must still be in impl lsp config env"
            );
        }
    }

    #[test]
    fn review_config_parses_workflow_and_defaults() {
        let c = RegistryConfig::parse(
            "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n[review]\nworkflow=\"implement-review\"\n",
        )
        .unwrap();
        let r = c.review.as_ref().unwrap().to_config().unwrap();
        assert_eq!(r.workflow.as_str(), "implement-review");
        assert_eq!(r.max_output_bytes, 16 * 1024);
        assert_eq!(r.timeout, std::time::Duration::from_secs(300));
    }

    #[test]
    fn review_config_defaults_workflow_when_absent() {
        let c = RegistryConfig::parse(
            "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n[review]\n",
        )
        .unwrap();
        assert_eq!(
            c.review
                .as_ref()
                .unwrap()
                .to_config()
                .unwrap()
                .workflow
                .as_str(),
            "implement-review"
        );
    }

    #[test]
    fn review_config_rejects_malformed_workflow_id() {
        let c = RegistryConfig::parse(
            "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n[review]\nworkflow=\"\"\n",
        )
        .unwrap();
        assert!(c.review.as_ref().unwrap().to_config().is_err());
    }

    #[test]
    fn implement_config_defaults_when_absent() {
        let lc = ImplementToml {
            max_attempts: None,
            fix_workflow: None,
            max_session_respawns: None,
        }
        .to_config()
        .unwrap();
        assert_eq!(lc.max_attempts, 3);
        assert_eq!(lc.fix_workflow.as_str(), "implement-fix");
        assert_eq!(lc.max_session_respawns, 3);
        assert_eq!(LoopConfig::default().max_attempts, 3);
        assert_eq!(LoopConfig::default().fix_workflow.as_str(), "implement-fix");
        assert_eq!(LoopConfig::default().max_session_respawns, 3);
    }

    #[test]
    fn implement_config_max_attempts_zero_is_error() {
        assert!(ImplementToml {
            max_attempts: Some(0),
            fix_workflow: None,
            max_session_respawns: None,
        }
        .to_config()
        .is_err());
    }

    #[test]
    fn implement_config_clamps_above_hard_max() {
        let lc = ImplementToml {
            max_attempts: Some(99),
            fix_workflow: None,
            max_session_respawns: None,
        }
        .to_config()
        .unwrap();
        assert_eq!(lc.max_attempts, 10);
    }

    #[test]
    fn implement_config_custom_fix_workflow_and_malformed() {
        let lc = ImplementToml {
            max_attempts: Some(2),
            fix_workflow: Some("my-fix".into()),
            max_session_respawns: None,
        }
        .to_config()
        .unwrap();
        assert_eq!(lc.max_attempts, 2);
        assert_eq!(lc.fix_workflow.as_str(), "my-fix");
        assert!(ImplementToml {
            max_attempts: None,
            fix_workflow: Some("".into()),
            max_session_respawns: None,
        }
        .to_config()
        .is_err());
    }

    #[test]
    fn implement_config_max_session_respawns_defaults_and_clamps() {
        let lc = ImplementToml {
            max_attempts: None,
            fix_workflow: None,
            max_session_respawns: Some(99),
        }
        .to_config()
        .unwrap();
        assert_eq!(lc.max_session_respawns, RESPAWN_HARD_MAX);

        let lc = ImplementToml {
            max_attempts: None,
            fix_workflow: None,
            max_session_respawns: Some(0),
        }
        .to_config()
        .unwrap();
        assert_eq!(lc.max_session_respawns, 0);
    }

    #[test]
    fn implement_block_parses_from_toml() {
        let c = RegistryConfig::parse(
            "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n\
             [implement]\nmax_attempts=2\nfix_workflow=\"implement-fix\"\nmax_session_respawns=4\n",
        )
        .unwrap();
        let lc = c.implement.as_ref().unwrap().to_config().unwrap();
        assert_eq!(lc.max_attempts, 2);
        assert_eq!(lc.max_session_respawns, 4);
        let c2 = RegistryConfig::parse(
            "default=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n[[agents]]\nid=\"x\"\ncmd=\"echo\"\n",
        )
        .unwrap();
        assert!(c2.implement.is_none());
    }

    #[test]
    fn api_entry_parses_without_cmd() {
        let toml = "default=\"ollama\"\n[[agents]]\nid=\"ollama\"\nkind=\"api\"\nbase_url=\"http://localhost:11434/v1\"\nmodel=\"qwen3.5:9b\"\n[server]\naddr=\"127.0.0.1:8080\"\n";
        let snap = RegistryConfig::parse(toml)
            .unwrap()
            .into_snapshot()
            .unwrap();
        let e = snap
            .entries
            .iter()
            .find(|e| e.id.as_str() == "ollama")
            .unwrap();
        assert!(e.cmd.is_none());
        assert_eq!(e.base_url.as_deref(), Some("http://localhost:11434/v1"));
        assert!(!snap.allowed_cmds.iter().any(|c| c.is_empty()));
    }

    #[test]
    fn api_entry_with_cmd_is_rejected() {
        let toml = "default=\"x\"\n[[agents]]\nid=\"x\"\nkind=\"api\"\nbase_url=\"http://h/v1\"\ncmd=\"nope\"\n[server]\naddr=\"127.0.0.1:8080\"\n";
        assert!(RegistryConfig::parse(toml)
            .unwrap()
            .into_snapshot()
            .is_err());
    }

    #[test]
    fn acp_entry_without_cmd_is_rejected() {
        let toml = "default=\"x\"\n[[agents]]\nid=\"x\"\nkind=\"acp\"\n[server]\naddr=\"127.0.0.1:8080\"\n";
        assert!(RegistryConfig::parse(toml)
            .unwrap()
            .into_snapshot()
            .is_err());
    }

    // -----------------------------------------------------------------------
    // Task 7 (W1): [[workflows]] boot-load config
    // -----------------------------------------------------------------------

    const AGENTS_HEADER: &str =
        "default = \"codex\"\n[[agents]]\nid = \"codex\"\ncmd = \"codex-acp\"\n";
    const SERVER_FOOTER: &str = "[server]\naddr = \"127.0.0.1:8080\"\n";

    #[test]
    fn parses_workflows_and_loads_prompts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p.md"), "review {{input}}").unwrap();
        let toml = format!(
            "{AGENTS_HEADER}\n[[workflows]]\nid = \"wf1\"\n\
            [[workflows.nodes]]\nid = \"only\"\nagent = \"codex\"\nprompt_file = \"p.md\"\ninputs = []\n{SERVER_FOOTER}"
        );
        let cfg = RegistryConfig::parse(&toml).unwrap();
        let wfs = cfg.load_workflows(dir.path()).unwrap();
        let g = wfs
            .get(&bridge_core::ids::WorkflowId::parse("wf1").unwrap())
            .unwrap();
        assert_eq!(g.nodes[0].prompt_template, "review {{input}}");
        g.validate().unwrap();
    }

    #[test]
    fn workflow_unknown_agent_rejected_at_boot() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p.md"), "x").unwrap();
        let toml = format!(
            "{AGENTS_HEADER}\n[[workflows]]\nid = \"wf1\"\n\
            [[workflows.nodes]]\nid = \"only\"\nagent = \"ghost\"\nprompt_file = \"p.md\"\ninputs = []\n{SERVER_FOOTER}"
        );
        assert!(RegistryConfig::parse(&toml)
            .unwrap()
            .load_workflows(dir.path())
            .is_err());
    }

    #[test]
    fn workflow_missing_prompt_file_fails_loud() {
        let dir = tempfile::tempdir().unwrap();
        let toml = format!(
            "{AGENTS_HEADER}\n[[workflows]]\nid = \"wf1\"\n\
            [[workflows.nodes]]\nid = \"only\"\nagent = \"codex\"\nprompt_file = \"nope.md\"\ninputs = []\n{SERVER_FOOTER}"
        );
        assert!(RegistryConfig::parse(&toml)
            .unwrap()
            .load_workflows(dir.path())
            .is_err());
    }

    #[test]
    fn workflow_bad_dag_fails_loud() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("p.md"), "x").unwrap();
        let toml = format!(
            "{AGENTS_HEADER}\n[[workflows]]\nid = \"wf1\"\n\
            [[workflows.nodes]]\nid = \"a\"\nagent = \"codex\"\nprompt_file = \"p.md\"\ninputs = []\n\
            [[workflows.nodes]]\nid = \"b\"\nagent = \"codex\"\nprompt_file = \"p.md\"\ninputs = []\n{SERVER_FOOTER}"
        );
        assert!(RegistryConfig::parse(&toml)
            .unwrap()
            .load_workflows(dir.path())
            .is_err());
    }
}

#[cfg(test)]
mod session_cwd_cfg_tests {
    use super::*;

    #[test]
    fn agent_session_cwd_and_allowed_root_parse() {
        let cfg: RegistryConfig = RegistryConfig::parse(
            "default=\"a\"\nallowed_cwd_root=\"/work\"\n[[agents]]\nid=\"a\"\ncmd=\"x\"\ncwd=\"/host\"\nsession_cwd=\"/work/r\"\n[server]\naddr=\"127.0.0.1:8080\"\n",
        ).unwrap();
        let a = cfg.agents.iter().find(|a| a.id == "a").unwrap();
        assert_eq!(a.cwd.as_deref(), Some("/host"));
        assert_eq!(a.session_cwd.as_deref(), Some("/work/r"));
        assert_eq!(cfg.allowed_cwd_root.as_deref(), Some("/work"));
        let cfg2: RegistryConfig = RegistryConfig::parse(
            "default=\"a\"\n[[agents]]\nid=\"a\"\ncmd=\"x\"\n[server]\naddr=\"127.0.0.1:8080\"\n",
        )
        .unwrap();
        assert_eq!(cfg2.agents[0].session_cwd, None);
        assert_eq!(cfg2.allowed_cwd_root, None);
    }
}

#[cfg(test)]
mod store_cfg_tests {
    use super::*;

    #[test]
    fn warm_idle_ttl_defaults_and_overrides() {
        let base = r#"
default = "a"
[[agents]]
id = "a"
cmd = "echo"
[server]
addr = "127.0.0.1:8080"
"#;
        let cfg: RegistryConfig = RegistryConfig::parse(base).unwrap();
        assert_eq!(cfg.server.warm_idle_ttl_secs, 1800);
        assert_eq!(cfg.server.warm_usage_warn_fraction, None);
        assert_eq!(cfg.server.compact_summarize_timeout_secs, None);

        let cfg2: RegistryConfig = RegistryConfig::parse(&format!(
            "{base}warm_idle_ttl_secs = 5\nwarm_usage_warn_fraction = 0.8\ncompact_summarize_timeout_secs = 7\n"
        ))
        .unwrap();
        assert_eq!(cfg2.server.warm_idle_ttl_secs, 5);
        assert_eq!(cfg2.server.warm_usage_warn_fraction, Some(0.8));
        assert_eq!(cfg2.server.compact_summarize_timeout_secs, Some(7));
    }

    #[test]
    fn permission_policy_and_timeout_parse_with_absent_defaults() {
        let base = r#"
default = "a"
[[agents]]
id = "a"
cmd = "echo"
[server]
addr = "127.0.0.1:8080"
"#;
        let cfg: RegistryConfig = RegistryConfig::parse(base).unwrap();
        assert_eq!(cfg.server.permission_policy, None);
        assert_eq!(cfg.server.permission_timeout_ms, None);

        let cfg2: RegistryConfig = RegistryConfig::parse(&format!(
            "{base}permission_policy = \"defer\"\npermission_timeout_ms = 5000\n"
        ))
        .unwrap();
        assert_eq!(cfg2.server.permission_policy.as_deref(), Some("defer"));
        assert_eq!(cfg2.server.permission_timeout_ms, Some(5000));
    }

    #[test]
    fn store_path_parses_when_present() {
        let toml = r#"
default = "codex"
[server]
addr = "127.0.0.1:8080"
[store]
path = "/tmp/x.db"
"#;
        let cfg = RegistryConfig::parse(toml).unwrap();
        assert_eq!(cfg.store.unwrap().path, "/tmp/x.db");
    }

    #[test]
    fn store_absent_is_none() {
        let toml = "default = \"codex\"\n[server]\naddr = \"127.0.0.1:8080\"\n";
        let cfg = RegistryConfig::parse(toml).unwrap();
        assert!(cfg.store.is_none());
    }

    #[test]
    fn store_resume_attempt_cap_parses_and_defaults() {
        // present: resume_attempt_cap=5 round-trips to Some(5).
        let cfg: RegistryConfig = RegistryConfig::parse(
            "default = \"codex\"\n[server]\naddr = \"127.0.0.1:8080\"\n[store]\npath = \"x.db\"\nresume_attempt_cap = 5\n",
        )
        .unwrap();
        assert_eq!(cfg.store.as_ref().unwrap().resume_attempt_cap, Some(5));

        // absent → None (the .unwrap_or(3) default is applied at the call site).
        let cfg2: RegistryConfig = RegistryConfig::parse(
            "default = \"codex\"\n[server]\naddr = \"127.0.0.1:8080\"\n[store]\npath = \"x.db\"\n",
        )
        .unwrap();
        assert_eq!(cfg2.store.as_ref().unwrap().resume_attempt_cap, None);
    }

    #[test]
    fn parse_kind_accepts_container_rw() {
        assert_eq!(
            super::parse_kind("container_rw").unwrap(),
            bridge_core::domain::AgentKind::ContainerRw
        );
    }

    #[test]
    fn parse_kind_error_lists_container_rw() {
        let err = super::parse_kind("nope").unwrap_err();
        assert!(
            format!("{err:?}").contains("acp|api|container_rw"),
            "got: {err:?}"
        );
    }

    // ---- MCP (ADR-0028) ----------------------------------------------------

    fn mcp_toml(name: &str, command: &str, args: &[&str]) -> super::McpToml {
        super::McpToml {
            name: name.into(),
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: vec![],
        }
    }

    #[test]
    fn mcp_delivery_auto_detects_from_cmd_basename() {
        use bridge_core::mcp::McpDelivery;
        let r = |cmd: &str| super::resolve_mcp_delivery(None, Some(cmd), true, "a").unwrap();
        assert_eq!(r("claude-agent-acp"), McpDelivery::Acp);
        assert_eq!(r("/usr/bin/codex-acp"), McpDelivery::CodexNative); // path-qualified → basename
        assert_eq!(r("kiro-cli"), McpDelivery::KiroNative);
    }

    #[test]
    fn mcp_delivery_explicit_override_and_invalid() {
        use bridge_core::mcp::McpDelivery;
        assert_eq!(
            super::resolve_mcp_delivery(Some("kiro_native"), Some("codex-acp"), true, "a").unwrap(),
            McpDelivery::KiroNative
        );
        assert!(super::resolve_mcp_delivery(Some("bogus"), None, true, "a").is_err());
    }

    #[test]
    fn mcp_delivery_unknown_cmd_errors_only_when_mcp_present() {
        // With MCP servers + an unrecognized cmd and no override → hard error (don't guess).
        assert!(super::resolve_mcp_delivery(None, Some("weird-agent"), true, "a").is_err());
        // No MCP servers → delivery is irrelevant, defaults to Acp (no error).
        assert_eq!(
            super::resolve_mcp_delivery(None, Some("weird-agent"), false, "a").unwrap(),
            bridge_core::mcp::McpDelivery::Acp
        );
    }

    #[test]
    fn build_mcp_specs_accepts_valid_and_substitutes_later() {
        let specs = super::build_mcp_specs(
            &[mcp_toml("prism", "/opt/prism", &["--repo", "{cwd}"])],
            "a",
        )
        .unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "prism");
        assert_eq!(
            specs[0].args,
            vec!["--repo".to_string(), "{cwd}".to_string()]
        );
    }

    #[test]
    fn build_mcp_specs_rejects_bad_inputs() {
        // duplicate name
        assert!(
            super::build_mcp_specs(&[mcp_toml("p", "/a", &[]), mcp_toml("p", "/b", &[])], "a")
                .is_err()
        );
        // brace in command
        assert!(super::build_mcp_specs(&[mcp_toml("p", "/a/{cwd}", &[])], "a").is_err());
        // non-{cwd} template in args
        assert!(super::build_mcp_specs(&[mcp_toml("p", "/a", &["{repo}"])], "a").is_err());
        // non-bare-key name (dot would break the `-c mcp_servers.<name>` path)
        assert!(super::build_mcp_specs(&[mcp_toml("p.x", "/a", &[])], "a").is_err());
        // empty name
        assert!(super::build_mcp_specs(&[mcp_toml("", "/a", &[])], "a").is_err());
    }

    #[test]
    fn is_toml_bare_key_rules() {
        assert!(super::is_toml_bare_key("prism"));
        assert!(super::is_toml_bare_key("a-b_9"));
        assert!(!super::is_toml_bare_key(""));
        assert!(!super::is_toml_bare_key("a.b"));
        assert!(!super::is_toml_bare_key("a b"));
    }

    #[test]
    fn kiro_native_mcp_with_sandbox_is_rejected_host_only() {
        // A containerized kiro (sandbox) + MCP servers -> host-only error (the bridge writes
        // ~/.kiro/agents on the host, which a container can't read).
        let toml = "default=\"k\"\nallowed_cwd_root=\"/work\"\n[[agents]]\nid=\"k\"\ncmd=\"kiro-cli\"\n\
                    [agents.sandbox]\nimage=\"img\"\nmount=\"/work\"\naccess=\"ro\"\negress=\"open\"\n\
                    [[agents.mcp]]\nname=\"prism\"\ncommand=\"/p\"\nargs=[\"--repo\",\"{cwd}\"]\n[server]\n";
        let cfg = super::RegistryConfig::parse(toml).expect("parses");
        let err = cfg.into_snapshot().unwrap_err();
        assert!(format!("{err:?}").contains("host-only"), "got: {err:?}");
    }

    #[test]
    fn kiro_native_mcp_host_run_is_accepted() {
        // Same kiro + MCP but NO sandbox (host-run) -> accepted, delivery resolved to KiroNative.
        let toml = "default=\"k\"\n[[agents]]\nid=\"k\"\ncmd=\"kiro-cli\"\n\
                    [[agents.mcp]]\nname=\"prism\"\ncommand=\"/p\"\nargs=[\"--repo\",\"{cwd}\"]\n[server]\n";
        let snap = super::RegistryConfig::parse(toml)
            .expect("parses")
            .into_snapshot()
            .expect("host kiro + mcp is valid");
        let k = snap.entries.iter().find(|e| e.id.as_str() == "k").unwrap();
        assert_eq!(k.mcp_delivery, bridge_core::mcp::McpDelivery::KiroNative);
        assert_eq!(k.mcp.len(), 1);
    }

    // ---- [merge] config (ADR-0027) ----

    #[test]
    fn merge_config_validation() {
        // both identity halves -> Some
        let raw = "default = \"x\"\nallowed_cwd_root = \"/x\"\n[server]\n[merge]\ntarget_ref = \"main\"\n\
                   author_name = \"Op\"\nauthor_email = \"op@x.com\"\n";
        let cfg = super::RegistryConfig::parse(raw).unwrap();
        let m = cfg.merge.as_ref().unwrap().to_config().unwrap();
        assert_eq!(m.target_ref.as_deref(), Some("main"));
        assert_eq!(m.author.as_ref().unwrap().email, "op@x.com");

        // half identity -> error
        let half = "default = \"x\"\n[server]\n[merge]\nauthor_name = \"Op\"\n";
        assert!(super::RegistryConfig::parse(half)
            .unwrap()
            .merge
            .as_ref()
            .unwrap()
            .to_config()
            .is_err());

        // empty target_ref -> error
        let empty = "default = \"x\"\n[server]\n[merge]\ntarget_ref = \"\"\n";
        assert!(super::RegistryConfig::parse(empty)
            .unwrap()
            .merge
            .as_ref()
            .unwrap()
            .to_config()
            .is_err());

        // absent [merge] -> None
        let none = "default = \"x\"\nallowed_cwd_root = \"/x\"\n[server]\n";
        assert!(super::RegistryConfig::parse(none).unwrap().merge.is_none());
    }

    // ---- [review] slice + threshold fields (Task 5) ----

    #[test]
    fn review_toml_parses_slice_and_thresholds_with_defaults() {
        let t: ReviewToml = toml::from_str("workflow = \"implement-review\"").unwrap();
        let c = t.to_config().unwrap();
        assert_eq!(c.light_max_lines, 15);
        assert_eq!(c.light_max_files, 2);
        assert_eq!(c.thorough_min_lines, 150);
        assert_eq!(c.thorough_min_files, 6);
        assert_eq!(c.default_depth, crate::review::Depth::Auto);
        assert!(c.slice_cmd.to_string_lossy().ends_with("prism"));
    }

    #[test]
    fn review_toml_rejects_unordered_bands() {
        let t: ReviewToml =
            toml::from_str("workflow=\"r\"\nlight_max_lines=200\nthorough_min_lines=100").unwrap();
        assert!(t.to_config().is_err());
    }

    #[test]
    fn review_toml_rejects_bogus_default_depth() {
        let t: ReviewToml = toml::from_str("workflow=\"r\"\ndefault_depth=\"bogus\"").unwrap();
        assert!(t.to_config().is_err());
    }

    #[test]
    fn review_toml_rejects_zero_thresholds() {
        let t: ReviewToml = toml::from_str("workflow=\"r\"\nlight_max_lines=0").unwrap();
        assert!(t.to_config().is_err());
    }

    #[test]
    fn review_toml_rejects_zero_slice_bounds() {
        let t: ReviewToml = toml::from_str("workflow=\"r\"\nslice_timeout_secs=0").unwrap();
        assert!(t.to_config().is_err());
        let t2: ReviewToml = toml::from_str("workflow=\"r\"\nslice_max_bytes=0").unwrap();
        assert!(t2.to_config().is_err());
    }

    #[test]
    fn review_toml_explicit_values_round_trip() {
        // Guards against `to_config` hard-coding a default instead of reading `self.<field>`.
        let t: ReviewToml = toml::from_str(
            "workflow=\"r\"\nslice_timeout_secs=30\nslice_max_bytes=4096\nlight_max_lines=100\nlight_max_files=9\nthorough_min_lines=250\nthorough_min_files=12\ndefault_depth=\"thorough\"",
        )
        .unwrap();
        let c = t.to_config().unwrap();
        assert_eq!(c.slice_timeout, std::time::Duration::from_secs(30));
        assert_eq!(c.slice_max_bytes, 4096);
        assert_eq!(c.light_max_lines, 100);
        assert_eq!(c.light_max_files, 9);
        assert_eq!(c.thorough_min_lines, 250);
        assert_eq!(c.thorough_min_files, 12);
        assert_eq!(
            c.default_depth,
            crate::review::Depth::Forced(crate::review::Tier::Thorough)
        );
    }

    #[test]
    fn review_slice_cmd_tilde_actually_expands() {
        // `ends_with("prism")` alone would pass even if expansion were a no-op; assert the real expansion.
        // Read the ambient $HOME (don't mutate global env — that races parallel tests).
        assert_eq!(super::shellexpand_tilde("/abs/prism"), "/abs/prism"); // non-tilde passes through
        if let Ok(home) = std::env::var("HOME") {
            let t: ReviewToml =
                toml::from_str("workflow=\"r\"\nslice_cmd=\"~/bin/prism\"").unwrap();
            let c = t.to_config().unwrap();
            assert_eq!(
                c.slice_cmd,
                std::path::PathBuf::from(format!("{home}/bin/prism"))
            );
        }
    }
}
