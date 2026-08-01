//! Locked, bounded projection of an already-sanitized failure diagnostic.
//!
//! Raw summaries, stderr, commands, paths, and arbitrary diagnostic collections
//! are intentionally absent from this DTO.

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct FailureDiagnosticWire {
    pub schema_version: u16,
    pub failed_phase: crate::diagnostics::DiagnosticPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_completed_phase: Option<crate::diagnostics::DiagnosticPhase>,
    pub class: crate::diagnostics::DiagnosticFailureClass,
    pub disposition: crate::diagnostics::FailureDisposition,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deepest_cause: Option<String>,
    pub prompt_may_have_been_accepted: bool,
}

impl From<&crate::diagnostics::FailureDiagnostic> for FailureDiagnosticWire {
    fn from(value: &crate::diagnostics::FailureDiagnostic) -> Self {
        Self {
            schema_version: value.schema_version(),
            failed_phase: value.failed_phase(),
            last_completed_phase: value.last_completed_phase(),
            class: value.class(),
            disposition: value.disposition(),
            code: value.code().as_str().to_owned(),
            deepest_cause: value.causes().last().cloned(),
            prompt_may_have_been_accepted: value.prompt_may_have_been_accepted(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{
        DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor, FailureDiagnostic,
        FailureDiagnosticInput, FailureDisposition,
    };

    #[test]
    fn projection_excludes_summary_stderr_and_all_but_deepest_cause() {
        let diagnostic = FailureDiagnostic::build_static_code(
            FailureDiagnosticInput {
                failed_phase: DiagnosticPhase::PromptStream,
                last_completed_phase: Some(DiagnosticPhase::ConfigApply),
                class: DiagnosticFailureClass::ProviderLimit,
                disposition: FailureDisposition::Fatal,
                code: String::new(),
                summary: "summary sentinel".into(),
                causes: vec!["outer cause".into(), "deepest cause".into()],
                stderr_observed: false,
                stderr_line_count: 0,
                stderr_scope: None,
                stderr_tail: None,
                stderr_redaction: None,
                retry_after_ms: None,
                reset_at_ms: None,
                prompt_may_have_been_accepted: true,
            },
            "provider.limit",
            &DiagnosticRedactor::default(),
        )
        .unwrap();
        let value = serde_json::to_value(FailureDiagnosticWire::from(&diagnostic)).unwrap();
        let encoded = value.to_string();
        assert_eq!(value["deepest_cause"], "deepest cause");
        for forbidden in ["summary sentinel", "outer cause", "stderr"] {
            assert!(!encoded.contains(forbidden), "{encoded}");
        }
    }
}
