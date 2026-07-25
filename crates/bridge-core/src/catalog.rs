//! Per-agent capability catalog: advertised models/effort/modes, probed live.

use std::collections::BTreeMap;

/// What one agent advertises. Empty vecs mean "the backend advertises none"
/// (e.g. kiro/api have no effort/modes) - renderers omit empty keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentCaps {
    pub current_model: Option<String>,
    pub models: Vec<String>,
    /// True when `a2a-bridge.model` can apply one of `models` for this agent.
    /// Kiro's native list is discoverable but not configurable through ACP SDK 1.x.
    pub model_configurable: bool,
    pub effort_levels: Vec<String>,
    pub modes: Vec<String>,
    pub current_mode: Option<String>,
}

/// agent_id -> caps. An agent that failed to probe is ABSENT (not a stub).
pub type ModelCatalog = BTreeMap<String, AgentCaps>;

/// Model-id markers intentionally blocked by this bridge even if an agent advertises them.
/// Blocked because these families have strict usage limits / short availability — the bridge
/// must not let them be pinned, defaulted, or (the real hazard) silently inherited from a
/// `~/.claude/settings.json` current-model into a bridge agent, which would burn the quota
/// unintentionally. See ADR-0029.
pub const BLOCKED_MODEL_MARKERS: &[&str] = &["fable"];

/// Master switch (default OFF): setting `A2A_BRIDGE_ALLOW_FABLE=1` (or `true`) at process start
/// unblocks fable-family ids for that invocation, so a deliberate `model = "fable"` pin /
/// `a2a-bridge.model=fable` override works and fable is advertised in the catalog. With the switch
/// OFF (the default), fable stays blocked EVERYWHERE — so it can never be used by accident; you have
/// to intentionally enable it. Read once (cached); the guard is invocation-level, not per-request.
fn fable_allowed() -> bool {
    use std::sync::OnceLock;
    static ALLOWED: OnceLock<bool> = OnceLock::new();
    *ALLOWED.get_or_init(|| {
        std::env::var("A2A_BRIDGE_ALLOW_FABLE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

pub fn is_blocked_model_id(model: &str) -> bool {
    is_blocked_model_id_gated(model, fable_allowed())
}

/// The pure block decision, parameterized on the master switch (so it is testable without env state).
/// The switch only lifts the `fable` marker — any OTHER marker added to `BLOCKED_MODEL_MARKERS`
/// stays enforced regardless of the switch (codex review: don't disable the whole blocklist).
fn is_blocked_model_id_gated(model: &str, fable_allowed: bool) -> bool {
    let model = model.to_ascii_lowercase();
    BLOCKED_MODEL_MARKERS
        .iter()
        .filter(|marker| !(fable_allowed && **marker == "fable"))
        .any(|marker| model.contains(marker))
}

pub fn sanitize_model_caps(mut caps: AgentCaps) -> AgentCaps {
    caps.models.retain(|model| !is_blocked_model_id(model));
    if caps
        .current_model
        .as_deref()
        .is_some_and(is_blocked_model_id)
    {
        caps.current_model = None;
    }
    if caps.models.is_empty() {
        caps.model_configurable = false;
    }
    caps
}

/// Parse `kiro-cli chat --list-models` text. Each model line is
/// `[*] <id> <multiplier>x credits  <description>`; the `*` marks the default.
/// Header lines (no model id) are skipped.
pub fn parse_kiro_list_models(stdout: &str) -> AgentCaps {
    let mut caps = AgentCaps::default();
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        let (is_default, rest) = match trimmed.strip_prefix('*') {
            Some(r) => (true, r.trim_start()),
            None => (false, trimmed),
        };
        // A model line's first token is the id and the line carries "credits".
        let Some(id) = rest.split_whitespace().next() else {
            continue;
        };
        if !rest.contains("credits") || id.is_empty() {
            continue; // header / blank / non-model line
        }
        if is_blocked_model_id(id) {
            continue;
        }
        caps.models.push(id.to_string());
        if is_default {
            caps.current_model = Some(id.to_string());
        }
    }
    caps
}

/// The per-agent JSON object the Agent Card extension and each successful `a2a-bridge models --json`
/// entry both emit (DRY). `current`/`models`/`model_configurable`/`current_mode` ride through; empty
/// `effort`/`modes` keys are OMITTED (no `"effort":[]` noise for kiro/api). The CLI may additionally
/// include explicit failure entries for agents whose live discovery probe failed.
pub fn caps_to_json(caps: &AgentCaps) -> serde_json::Value {
    let caps = sanitize_model_caps(caps.clone());
    let mut object = serde_json::Map::new();
    if let Some(model) = &caps.current_model {
        object.insert("current".into(), serde_json::json!(model));
    }
    object.insert("models".into(), serde_json::json!(caps.models));
    if !caps.models.is_empty() {
        object.insert(
            "model_configurable".into(),
            serde_json::json!(caps.model_configurable),
        );
    }
    if !caps.effort_levels.is_empty() {
        object.insert("effort".into(), serde_json::json!(caps.effort_levels));
    }
    if !caps.modes.is_empty() {
        object.insert("modes".into(), serde_json::json!(caps.modes));
    }
    if let Some(mode) = &caps.current_mode {
        object.insert("current_mode".into(), serde_json::json!(mode));
    }
    serde_json::Value::Object(object)
}

/// Parse an OpenAI-compatible `GET /v1/models` body -> model ids (in `data[].id` order).
pub fn parse_ollama_models(body: &str) -> Result<AgentCaps, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct Entry {
        id: String,
    }

    #[derive(serde::Deserialize)]
    struct List {
        data: Vec<Entry>,
    }

    let list: List = serde_json::from_str(body)?;
    Ok(sanitize_model_caps(AgentCaps {
        models: list.data.into_iter().map(|e| e.id).collect(),
        model_configurable: true,
        ..Default::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fable_gate_blocks_by_default_and_allows_when_switched_on() {
        // Default (switch OFF): fable-family ids are blocked (identity of raw + `[1m]` variants).
        assert!(is_blocked_model_id_gated("claude-fable-5", false));
        assert!(is_blocked_model_id_gated("claude-fable-5.1[1m]", false));
        assert!(is_blocked_model_id_gated("FABLE", false));
        // Non-fable models are never blocked, switch either way.
        assert!(!is_blocked_model_id_gated("claude-sonnet-4.5", false));
        assert!(!is_blocked_model_id_gated("claude-sonnet-4.5", true));
        // Switch ON: fable-family ids are permitted (the deliberate A2A_BRIDGE_ALLOW_FABLE window).
        assert!(!is_blocked_model_id_gated("claude-fable-5", true));
        assert!(!is_blocked_model_id_gated("claude-fable-5.1[1m]", true));
    }

    #[test]
    fn caps_default_is_empty() {
        let c = AgentCaps::default();
        assert!(c.models.is_empty() && c.effort_levels.is_empty() && c.modes.is_empty());
        assert!(c.current_model.is_none());
    }

    #[test]
    fn parses_kiro_list_models() {
        let out = "Available models (* = default):\n\n* auto                 1.00x credits      Models chosen by task\n  claude-sonnet-4.5    1.30x credits      The Claude Sonnet 4.5 model\n  claude-fable-5.1     1.00x credits      Blocked model\n  claude-haiku-4.5     0.40x credits      The latest Claude Haiku model\n";
        let caps = parse_kiro_list_models(out);
        assert_eq!(
            caps.models,
            vec!["auto", "claude-sonnet-4.5", "claude-haiku-4.5"]
        );
        assert_eq!(caps.current_model.as_deref(), Some("auto"));
        assert!(!caps.model_configurable);
        assert!(caps.effort_levels.is_empty() && caps.modes.is_empty());
    }

    #[test]
    fn parses_ollama_models_list() {
        let body = r#"{"object":"list","data":[{"id":"qwen2.5-coder:7b","object":"model"},{"id":"claude-fable-5.1[1m]","object":"model"},{"id":"llama3.1:8b","object":"model"}]}"#;
        let caps = parse_ollama_models(body).expect("valid list");
        assert_eq!(caps.models, vec!["qwen2.5-coder:7b", "llama3.1:8b"]);
        assert!(caps.model_configurable);
        assert!(caps.current_model.is_none() && caps.effort_levels.is_empty());
    }

    #[test]
    fn parse_ollama_models_all_blocked_disables_model_configurable() {
        let body = r#"{"object":"list","data":[{"id":"claude-fable-5.1[1m]","object":"model"}]}"#;
        let caps = parse_ollama_models(body).expect("valid list");
        assert!(caps.models.is_empty());
        assert!(!caps.model_configurable);
    }

    #[test]
    fn ollama_models_rejects_garbage() {
        assert!(parse_ollama_models("not json").is_err());
    }

    #[test]
    fn caps_to_json_emits_present_keys_and_omits_empty() {
        let caps = AgentCaps {
            current_model: Some("sonnet".into()),
            models: vec![
                "default".into(),
                "claude-fable-5.1[1m]".into(),
                "sonnet".into(),
            ],
            model_configurable: true,
            effort_levels: vec!["low".into(), "high".into()],
            modes: vec![],
            current_mode: None,
        };
        let value = caps_to_json(&caps);
        assert_eq!(value["current"], serde_json::json!("sonnet"));
        assert_eq!(value["models"], serde_json::json!(["default", "sonnet"]));
        assert_eq!(value["model_configurable"], serde_json::json!(true));
        assert_eq!(value["effort"], serde_json::json!(["low", "high"]));
        assert!(value.get("modes").is_none(), "empty modes omitted");
        assert!(
            value.get("current_mode").is_none(),
            "absent current_mode omitted"
        );
    }

    #[test]
    fn caps_to_json_omits_effort_for_kiro_like_caps() {
        // kiro/api advertise models only — no effort, no modes, no current_mode keys.
        let caps = AgentCaps {
            current_model: Some("auto".into()),
            models: vec!["auto".into(), "glm-5".into()],
            ..Default::default()
        };
        let value = caps_to_json(&caps);
        assert!(value.get("effort").is_none());
        assert!(value.get("modes").is_none());
        assert_eq!(value["models"], serde_json::json!(["auto", "glm-5"]));
        assert_eq!(value["model_configurable"], serde_json::json!(false));
    }
}
