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

    /// ALL differing fields (order-independent). Slice 1 routes on the full set so a multi-field
    /// delta (e.g. model+cwd) is never partially reconciled.
    pub fn diff(&self, other: &SessionSpecFingerprint) -> Vec<&'static str> {
        let mut d = Vec::new();
        if self.agent != other.agent {
            d.push("agent");
        }
        if self.config.model != other.config.model {
            d.push("model");
        }
        if self.config.effort != other.config.effort {
            d.push("effort");
        }
        if self.config.mode != other.config.mode {
            d.push("mode");
        }
        if self.cwd != other.cwd {
            d.push("cwd");
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::Effort;

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

    fn fp_full(
        agent: &str,
        model: &str,
        effort: Option<Effort>,
        mode: Option<&str>,
        cwd: Option<&str>,
    ) -> SessionSpecFingerprint {
        SessionSpecFingerprint {
            agent: AgentId::parse(agent).unwrap(),
            config: EffectiveConfig {
                model: Some(model.into()),
                effort,
                mode: mode.map(|s| s.to_string()),
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

    #[test]
    fn diff_returns_all_mismatched_fields() {
        let a = fp_full(
            "codex",
            "gpt-5.5",
            Some(Effort::High),
            Some("default"),
            Some("/a"),
        );
        let b = fp_full(
            "codex",
            "gpt-5.4",
            Some(Effort::Medium),
            Some("default"),
            Some("/b"),
        );

        let mut diff = a.diff(&b);
        diff.sort_unstable();
        assert_eq!(diff, vec!["cwd", "effort", "model"]);
        assert!(!diff.contains(&"agent"));
        assert!(!diff.contains(&"mode"));
        assert!(a.diff(&a).is_empty());
    }
}
