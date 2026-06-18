//! Frozen-at-mint fingerprint for warm-session continuation. A `continue` whose recomputed
//! fingerprint differs -> typed `ConfigMismatch{field}` (Slice 0; reconcile is Slice 1).
use crate::domain::EffectiveConfig;
use crate::ids::AgentId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSpecFingerprint {
    pub agent: AgentId,
    pub config: EffectiveConfig,
    /// Canonical cwd string (None = no override). String (not SessionCwd) to avoid coupling
    /// to its derives; cwd is immutable post-`session/new`.
    pub cwd: Option<String>,
}

impl SessionSpecFingerprint {
    /// The first differing field (`agent`/`model`/`effort`/`mode`/`cwd`), else `None`.
    pub fn first_mismatch(&self, other: &SessionSpecFingerprint) -> Option<&'static str> {
        if self.agent != other.agent {
            return Some("agent");
        }
        if self.config.model != other.config.model {
            return Some("model");
        }
        if self.config.effort != other.config.effort {
            return Some("effort");
        }
        if self.config.mode != other.config.mode {
            return Some("mode");
        }
        if self.cwd != other.cwd {
            return Some("cwd");
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(model: &str, cwd: Option<&str>) -> SessionSpecFingerprint {
        SessionSpecFingerprint {
            agent: AgentId::parse("codex").unwrap(),
            config: EffectiveConfig {
                model: Some(model.into()),
                effort: None,
                mode: None,
            },
            cwd: cwd.map(|s| s.to_string()),
        }
    }

    #[test]
    fn identical_have_no_mismatch() {
        assert_eq!(
            fp("gpt-5.5", Some("/work")).first_mismatch(&fp("gpt-5.5", Some("/work"))),
            None
        );
    }

    #[test]
    fn model_and_cwd_mismatches_reported() {
        assert_eq!(
            fp("gpt-5.5", None).first_mismatch(&fp("gpt-5.4", None)),
            Some("model")
        );
        assert_eq!(
            fp("gpt-5.5", Some("/a")).first_mismatch(&fp("gpt-5.5", Some("/b"))),
            Some("cwd")
        );
    }
}
