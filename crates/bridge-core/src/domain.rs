// domain.rs — minimal shared domain value types (spec §5.2/§5.3).

use crate::ids::{AgentId, CallerId, ContextId};
use std::collections::BTreeMap;
use std::str::FromStr;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Part {
    pub text: String,
}

#[derive(Debug, Default, Clone)]
pub struct Artifact;

#[derive(Debug, Default, Clone)]
pub struct PromptOutcome;

/// Effort tier for agent execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl FromStr for Effort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.to_ascii_lowercase();
        match normalized.as_str() {
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            _ => Err(format!(
                "invalid effort: {s:?} (expected minimal/low/medium/high/xhigh/max)"
            )),
        }
    }
}

/// Which adapter implementation backs an agent entry. Parsed from the TOML `kind`
/// string in `bin/a2a-bridge/src/config.rs` (like `Effort`), defaulting to `Acp`.
/// Single-variant today; a 2nd kind (B1 `ClaudeApi`) re-expands the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentKind {
    #[default]
    Acp,
    /// Non-process OpenAI-compatible HTTP backend (bridge-api).
    Api,
    /// Write-capable per-turn containerized ACP agent (bridge-container, Slice B2a).
    ContainerRw,
}

/// How a containerized agent is launched (the enforced `[sandbox]` block, Slice B1). The bridge
/// composes the runtime argv from this (see [`crate::sandbox::compose_sandbox`]) and the registry/config
/// layers enforce its invariants, so containment can't silently degrade via hand-typed args.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxConfig {
    /// Container runtime; resolve via [`SandboxConfig::runtime`] (`"docker"` default). e.g. docker|podman.
    pub runtime: Option<String>,
    pub image: String,
    /// The primary identical-path source mount; MUST equal `allowed_cwd_root` (parse-layer S2). Stored
    /// NORMALIZED (via `SessionCwd`) so the snapshot-layer volume check (S6) compares like-for-like.
    pub mount: String,
    pub access: MountAccess,
    /// Data-carrying so `compose_sandbox` is total (the old runtime "Locked ⇒ network+proxy" is a type
    /// guarantee).
    pub egress: EgressPolicy,
    /// Verbatim extra `-v` specs (creds / named volumes); trusted passthrough. The primary `mount` is
    /// structurally validated; volume destinations may not nest under `mount` (S6).
    pub volumes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogConfig {
    pub idle_timeout: std::time::Duration,
    pub hard_wall_clock: std::time::Duration,
}

/// The container runtime used when none is configured. The SINGLE source of this literal — the sandbox
/// runtime resolver, the default-`allowed_cmds` union, and the `[verify]` runtime gate all read it, so the
/// "validate vs spawn" pair can never drift on the default.
pub const DEFAULT_RUNTIME: &str = "docker";

impl SandboxConfig {
    /// The resolved container runtime program (default [`DEFAULT_RUNTIME`]). The single source of truth: the
    /// snapshot-layer allowlist (S3) gates THIS value, and `compose_sandbox` spawns THIS — so validate
    /// and spawn can't drift.
    pub fn runtime(&self) -> &str {
        self.runtime.as_deref().unwrap_or(DEFAULT_RUNTIME)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountAccess {
    Ro,
    Rw,
}

/// `Locked` CARRIES its network/proxy so `compose_sandbox` is total (no `unwrap`/panic). The TOML→enum
/// conversion (`config.rs::parse_egress`) is the only constructor and rejects `locked` without both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressPolicy {
    Locked {
        network: String,
        proxy: String,
        no_proxy: Option<String>,
    },
    Open,
}

/// A named bundle: which CLI adapter to launch + model/effort/mode configuration.
#[derive(Debug, Clone)]
pub struct AgentEntry {
    pub id: AgentId,
    /// Process command for `kind="acp"`; `None` for non-process kinds (e.g. `Api`).
    pub cmd: Option<String>,
    /// OpenAI-compatible base URL for `kind="api"`; `None` for process kinds.
    pub base_url: Option<String>,
    /// NAME of the env var holding the bearer token for `kind="api"` (never the secret).
    pub api_key_env: Option<String>,
    pub args: Vec<String>,
    /// Adapter kind (selects the backend factory arm). Default `Acp`.
    pub kind: AgentKind,
    pub model_provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub mode: Option<String>,
    pub cwd: Option<String>,
    /// Static ACP session cwd for this agent (the working directory set at session mint).
    /// Resolution chain at mint: `session_cwd` → `cwd` → `"."`.
    /// This is NOT a host-process cwd — the host child has no cwd (Supervised gets None).
    pub session_cwd: Option<String>,
    /// The enforced `[sandbox]` block (B1): how to containerize this agent. `None` = raw `cmd`/`args`.
    pub sandbox: Option<SandboxConfig>,
    /// Optional per-agent E9 watchdog. `None` preserves the pre-watchdog turn behavior.
    pub watchdog: Option<WatchdogConfig>,
    /// MCP servers to offer this agent (ADR-0028). Empty = none. Delivered via [`Self::mcp_delivery`].
    pub mcp: Vec<crate::mcp::McpServerSpec>,
    /// Which channel delivers `mcp` to this agent (resolved at config build from `cmd`). Irrelevant
    /// when `mcp` is empty; defaults to `Acp`.
    pub mcp_delivery: crate::mcp::McpDelivery,
    pub auth_method: Option<String>,
    /// The launched ACP process already has ambient credentials; do not invoke
    /// an advertised interactive authentication method during initialization.
    pub pre_authenticated: bool,
    /// Whether this unsandboxed ACP entry may be named as a target in a local R2d fallback plan.
    /// This capability never asserts trust and never authorizes or performs execution.
    pub host_fallback_eligible: bool,
    pub name: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub version: Option<String>,
    pub extensions: BTreeMap<String, toml::Value>,
}

/// Per-request overrides that layer on top of an `AgentEntry`'s defaults.
#[derive(Debug, Clone, Default)]
pub struct AgentOverride {
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub mode: Option<String>,
}

/// Per-session config the backend applies at ACP mint.
/// `model` is the agent-native id (NO `{provider}@{model}`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveConfig {
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub mode: Option<String>,
}

/// Per-session stash carried through `configure_session` → `ensure_session`.
///
/// `config` holds model/mode/effort (what the LLM is configured with).
/// `cwd` holds the session working directory (a session *location*, not LLM config).
/// They are separate so future increments can set cwd independently of model config.
#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub config: EffectiveConfig,
    pub cwd: Option<crate::session_cwd::SessionCwd>,
}

impl SessionSpec {
    /// Convenience constructor for call sites that only carry config (cwd is `None`).
    pub fn from_config(config: EffectiveConfig) -> Self {
        Self { config, cwd: None }
    }
}

/// Compute effective config by layering an optional override on top of an entry's defaults.
/// Override fields take precedence when `Some`; `None` fields fall back to entry defaults.
pub fn effective_config(entry: &AgentEntry, ov: Option<&AgentOverride>) -> EffectiveConfig {
    let mut eff = EffectiveConfig {
        model: entry.model.clone(),
        effort: entry.effort,
        mode: entry.mode.clone(),
    };
    if let Some(o) = ov {
        if o.model.is_some() {
            eff.model = o.model.clone();
        }
        if o.effort.is_some() {
            eff.effort = o.effort;
        }
        if o.mode.is_some() {
            eff.mode = o.mode.clone();
        }
    }
    eff
}

/// Immutable snapshot of the registry state.
#[derive(Debug, Clone)]
pub struct RegistrySnapshot {
    pub default: AgentId,
    pub entries: Vec<AgentEntry>,
    pub allowed_cmds: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskMeta {
    pub skill: Option<String>,
    pub agent: Option<AgentId>,
    pub overrides: Option<AgentOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerTaskId(pub String);

#[derive(Debug, Clone)]
pub enum RouteTarget {
    Local(crate::ids::AgentId),
    Delegate,
    Fanout,
    Workflow(crate::ids::WorkflowId),
}

// --- Types added by Task 4 ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingKind {
    Permission,
    Auth,
}

#[derive(Debug, Clone)]
pub struct PendingRequest {
    pub request_id: String,
    pub kind: PendingKind,
}

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub request_id: String,
    pub interactive: bool,
}

impl PermissionRequest {
    pub fn read() -> Self {
        Self {
            request_id: String::new(),
            interactive: false,
        }
    }
    pub fn interactive() -> Self {
        Self {
            request_id: String::new(),
            interactive: true,
        }
    }
    pub fn with_id(request_id: impl Into<String>, interactive: bool) -> Self {
        Self {
            request_id: request_id.into(),
            interactive,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Approve,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "decision",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PermitDecision {
    Approve {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        option_id: Option<String>,
    },
    Deny {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        option_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Modify {
        option_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Escalate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectMode {
    PrependNextTurn,
    AppendNextTurn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedInject {
    pub text: String,
    pub mode: InjectMode,
    pub dedupe_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectRequest {
    pub context: ContextId,
    pub text: String,
    pub mode: InjectMode,
    pub dedupe_key: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionContext;

impl SessionContext {
    pub fn test() -> Self {
        Self
    }
}

#[derive(Debug, Clone)]
pub struct InboundRequest {
    pub token: Option<String>,
}

impl InboundRequest {
    pub fn anon() -> Self {
        Self { token: None }
    }
    pub fn with_token(t: &str) -> Self {
        Self {
            token: Some(t.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    caller: CallerId,
}

impl AuthContext {
    pub fn new(caller: CallerId) -> Self {
        Self { caller }
    }
    pub fn caller_id(&self) -> &CallerId {
        &self.caller
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_kind_defaults_to_acp() {
        assert_eq!(AgentKind::default(), AgentKind::Acp);
    }

    #[test]
    fn effort_from_str_parses_all_levels_case_insensitively() {
        for (s, expected) in [
            ("minimal", Effort::Minimal),
            ("LOW", Effort::Low),
            ("medium", Effort::Medium),
            ("High", Effort::High),
            ("xhigh", Effort::Xhigh),
            ("max", Effort::Max),
        ] {
            assert_eq!(s.parse::<Effort>().unwrap(), expected, "failed for {s:?}");
        }
    }

    #[test]
    fn effort_from_str_rejects_invalid() {
        let err = "bogus".parse::<Effort>().unwrap_err();
        assert!(err.contains("minimal/low/medium/high/xhigh/max"));
    }

    #[test]
    fn agent_entry_cmd_is_optional_and_has_url_fields() {
        let e = AgentEntry {
            id: AgentId::parse("ollama").unwrap(),
            cmd: None,
            args: vec![],
            kind: AgentKind::Api,
            base_url: Some("http://localhost:11434/v1".into()),
            api_key_env: None,
            model_provider: None,
            model: None,
            effort: None,
            mode: None,
            cwd: None,
            session_cwd: None,
            sandbox: None,
            watchdog: None,
            auth_method: None,
            pre_authenticated: false,
            host_fallback_eligible: false,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            mcp: vec![],
            mcp_delivery: Default::default(),
            extensions: Default::default(),
        };
        assert!(e.cmd.is_none());
        assert_eq!(e.base_url.as_deref(), Some("http://localhost:11434/v1"));
        assert_eq!(e.kind, AgentKind::Api);
    }

    #[test]
    fn agent_entry_carries_kind() {
        let e = AgentEntry {
            id: AgentId::parse("x").unwrap(),
            cmd: Some("codex-acp".into()),
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
            auth_method: None,
            pre_authenticated: false,
            host_fallback_eligible: false,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            mcp: vec![],
            mcp_delivery: Default::default(),
            extensions: Default::default(),
        };
        assert_eq!(e.kind, AgentKind::Acp);
    }

    #[test]
    fn effective_config_layers_override_over_entry() {
        let entry = AgentEntry {
            id: crate::ids::AgentId::parse("codex").unwrap(),
            cmd: Some("codex-acp".into()),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind: AgentKind::Acp,
            model_provider: Some("openai".into()),
            model: Some("gpt-5.5".into()),
            effort: Some(Effort::High),
            mode: Some("read-only".into()),
            cwd: None,
            session_cwd: None,
            sandbox: None,
            watchdog: None,
            auth_method: None,
            pre_authenticated: false,
            host_fallback_eligible: false,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            mcp: vec![],
            mcp_delivery: Default::default(),
            extensions: Default::default(),
        };
        let ov = AgentOverride {
            model: Some("gpt-5.4".into()),
            effort: None,
            mode: Some("auto".into()),
        };
        let eff = effective_config(&entry, Some(&ov));
        assert_eq!(eff.model.as_deref(), Some("gpt-5.4")); // override wins
        assert_eq!(eff.effort, Some(Effort::High)); // base kept (override None)
        assert_eq!(eff.mode.as_deref(), Some("auto")); // override wins
        let base = effective_config(&entry, None);
        assert_eq!(base.model.as_deref(), Some("gpt-5.5")); // base when no override
        assert_eq!(base.effort, Some(Effort::High));
    }

    #[test]
    fn permit_decision_variants_round_trip() {
        for d in [
            PermitDecision::Approve { option_id: None },
            PermitDecision::Approve {
                option_id: Some("approved".into()),
            },
            PermitDecision::Deny {
                option_id: None,
                reason: Some("nope".into()),
            },
            PermitDecision::Modify {
                option_id: "approved-execpolicy-amendment".into(),
                note: None,
            },
            PermitDecision::Escalate { reason: None },
        ] {
            let s = serde_json::to_string(&d).unwrap();
            assert_eq!(serde_json::from_str::<PermitDecision>(&s).unwrap(), d);
        }
    }

    #[test]
    fn inject_request_holds_mode() {
        let r = InjectRequest {
            context: ContextId::parse("c").unwrap(),
            text: "hi".into(),
            mode: InjectMode::PrependNextTurn,
            dedupe_key: None,
        };
        assert_eq!(r.mode, InjectMode::PrependNextTurn);
    }
}

#[cfg(test)]
mod v25 {
    use super::*;
    use crate::ids::{AgentId, CallerId};

    #[test]
    fn part_carries_text() {
        assert_eq!(Part { text: "hi".into() }.text, "hi");
    }
    #[test]
    fn task_meta_skill() {
        assert_eq!(
            TaskMeta {
                skill: Some("delegate".into()),
                ..Default::default()
            }
            .skill
            .as_deref(),
            Some("delegate")
        );
    }
    #[test]
    fn peer_task_id_holds_string() {
        let p = PeerTaskId("peer-1".into());
        assert_eq!(p.0, "peer-1");
    }
    #[test]
    fn route_target_delegate_variant() {
        let r = RouteTarget::Delegate;
        assert!(matches!(r, RouteTarget::Delegate));
    }
    #[test]
    fn route_target_fanout_variant() {
        let r = RouteTarget::Fanout;
        assert!(matches!(r, RouteTarget::Fanout));
    }
    #[test]
    fn auth_context_roundtrips_caller() {
        let caller = CallerId::parse("alice").unwrap();
        let ctx = AuthContext::new(caller.clone());
        assert_eq!(ctx.caller_id().as_str(), "alice");
    }
    #[test]
    fn inbound_request_with_token() {
        let req = InboundRequest::with_token("tok-123");
        assert_eq!(req.token.as_deref(), Some("tok-123"));
    }
    #[test]
    fn permission_request_read_is_non_interactive() {
        let req = PermissionRequest::read();
        assert!(!req.interactive);
        assert!(req.request_id.is_empty());
    }
    #[test]
    fn session_context_test_ctor() {
        let _ctx = SessionContext::test();
    }
    #[test]
    fn route_target_local() {
        let r = RouteTarget::Local(AgentId::parse("kiro").unwrap());
        assert!(matches!(r, RouteTarget::Local(a) if a.as_str() == "kiro"));
    }
}
