//! Per-agent capability catalog: advertised models/effort/modes, probed live.

use std::collections::BTreeMap;

/// What one agent advertises. Empty vecs mean "the backend advertises none"
/// (e.g. kiro/api have no effort/modes) - renderers omit empty keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentCaps {
    pub current_model: Option<String>,
    pub models: Vec<String>,
    pub effort_levels: Vec<String>,
    pub modes: Vec<String>,
    pub current_mode: Option<String>,
}

/// agent_id -> caps. An agent that failed to probe is ABSENT (not a stub).
pub type ModelCatalog = BTreeMap<String, AgentCaps>;

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
        caps.models.push(id.to_string());
        if is_default {
            caps.current_model = Some(id.to_string());
        }
    }
    caps
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
    Ok(AgentCaps {
        models: list.data.into_iter().map(|e| e.id).collect(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_default_is_empty() {
        let c = AgentCaps::default();
        assert!(c.models.is_empty() && c.effort_levels.is_empty() && c.modes.is_empty());
        assert!(c.current_model.is_none());
    }

    #[test]
    fn parses_kiro_list_models() {
        let out = "Available models (* = default):\n\n* auto                 1.00x credits      Models chosen by task\n  claude-sonnet-4.5    1.30x credits      The Claude Sonnet 4.5 model\n  claude-haiku-4.5     0.40x credits      The latest Claude Haiku model\n";
        let caps = parse_kiro_list_models(out);
        assert_eq!(
            caps.models,
            vec!["auto", "claude-sonnet-4.5", "claude-haiku-4.5"]
        );
        assert_eq!(caps.current_model.as_deref(), Some("auto"));
        assert!(caps.effort_levels.is_empty() && caps.modes.is_empty());
    }

    #[test]
    fn parses_ollama_models_list() {
        let body = r#"{"object":"list","data":[{"id":"qwen2.5-coder:7b","object":"model"},{"id":"llama3.1:8b","object":"model"}]}"#;
        let caps = parse_ollama_models(body).expect("valid list");
        assert_eq!(caps.models, vec!["qwen2.5-coder:7b", "llama3.1:8b"]);
        assert!(caps.current_model.is_none() && caps.effort_levels.is_empty());
    }

    #[test]
    fn ollama_models_rejects_garbage() {
        assert!(parse_ollama_models("not json").is_err());
    }
}
