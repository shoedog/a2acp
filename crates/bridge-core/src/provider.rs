//! Closed, schema-v1 provider diagnostic evidence shared by HTTP and ACP adapters.

use crate::diagnostics::DiagnosticFailureClass;
use serde_json::Value;

pub const MAX_PROVIDER_RETRY_AFTER_MS: u64 = 2_592_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderEvidence {
    pub class: DiagnosticFailureClass,
    pub code: &'static str,
    pub retry_after_ms: Option<u64>,
    pub reset_at_ms: Option<i64>,
}

pub fn provider_token_class(token: &str) -> Option<DiagnosticFailureClass> {
    Some(match token {
        "insufficient_quota"
        | "quota_exceeded"
        | "billing_hard_limit_reached"
        | "usage_limit_reached"
        | "usage_limit_exceeded"
        | "rate_limit_exceeded"
        | "rate_limit_error" => DiagnosticFailureClass::ProviderLimit,
        "overloaded_error"
        | "server_overloaded"
        | "capacity_exceeded"
        | "temporarily_unavailable" => DiagnosticFailureClass::Overloaded,
        "authentication_error" | "invalid_api_key" | "permission_error" => {
            DiagnosticFailureClass::Authentication
        }
        "model_not_found" | "invalid_model" | "unsupported_model" => DiagnosticFailureClass::Model,
        _ => return None,
    })
}

pub fn provider_code(class: DiagnosticFailureClass) -> &'static str {
    match class {
        DiagnosticFailureClass::Authentication => "upstream.authentication",
        DiagnosticFailureClass::Model => "upstream.model",
        DiagnosticFailureClass::Overloaded => "upstream.overloaded",
        DiagnosticFailureClass::ProviderLimit => "upstream.provider_limit",
        DiagnosticFailureClass::Config
        | DiagnosticFailureClass::Protocol
        | DiagnosticFailureClass::Transport
        | DiagnosticFailureClass::AgentProcess
        | DiagnosticFailureClass::ContainerRuntime
        | DiagnosticFailureClass::ContainerImage
        | DiagnosticFailureClass::ContainerNetwork
        | DiagnosticFailureClass::ContainerMount
        | DiagnosticFailureClass::ContainerCredentials
        | DiagnosticFailureClass::Timeout
        | DiagnosticFailureClass::Persistence
        | DiagnosticFailureClass::Canceled
        | DiagnosticFailureClass::Unknown => "upstream.unknown",
    }
}

fn values_at_paths<'a>(data: &'a serde_json::Map<String, Value>, key: &str) -> Vec<&'a Value> {
    let mut values = Vec::with_capacity(2);
    if let Some(value) = data.get(key) {
        values.push(value);
    }
    if let Some(nested) = data.get("error").and_then(Value::as_object) {
        if let Some(value) = nested.get(key) {
            values.push(value);
        }
    }
    values
}

fn retry_hint(values: Vec<&Value>) -> (Option<u64>, bool) {
    if values.is_empty() {
        return (None, false);
    }
    if values.len() != 1 {
        return (None, true);
    }
    match values[0]
        .as_u64()
        .filter(|value| *value <= MAX_PROVIDER_RETRY_AFTER_MS)
    {
        Some(value) => (Some(value), false),
        None => (None, true),
    }
}

fn reset_hint(values: Vec<&Value>, now_ms: i64) -> (Option<i64>, bool) {
    if values.is_empty() {
        return (None, false);
    }
    if values.len() != 1 {
        return (None, true);
    }
    let horizon = now_ms.checked_add(MAX_PROVIDER_RETRY_AFTER_MS as i64);
    match values[0]
        .as_i64()
        .filter(|value| *value >= 0)
        .filter(|value| horizon.is_some_and(|horizon| *value <= horizon))
    {
        Some(value) => (Some(value), false),
        None => (None, true),
    }
}

pub fn classify_acp_error_data(
    auth_required: bool,
    data: Option<&Value>,
    now_ms: i64,
) -> ProviderEvidence {
    let object = data.and_then(Value::as_object);
    let mut token_classes = object.into_iter().flat_map(|data| {
        ["code", "type"]
            .into_iter()
            .flat_map(|key| values_at_paths(data, key))
            .filter_map(Value::as_str)
            .filter_map(provider_token_class)
    });

    let (class, classification_conflict) = if auth_required {
        (DiagnosticFailureClass::Authentication, false)
    } else {
        match token_classes.next() {
            None => (DiagnosticFailureClass::Unknown, false),
            Some(first) => {
                let conflict = token_classes.any(|class| class != first);
                if conflict {
                    (DiagnosticFailureClass::Unknown, true)
                } else {
                    (first, false)
                }
            }
        }
    };

    let (retry_after_ms, retry_invalid) = object.map_or((None, false), |data| {
        retry_hint(values_at_paths(data, "retry_after_ms"))
    });
    let (reset_at_ms, reset_invalid) = object.map_or((None, false), |data| {
        reset_hint(values_at_paths(data, "reset_at_ms"), now_ms)
    });
    let code = if classification_conflict {
        "upstream.classification_conflict"
    } else if retry_invalid || reset_invalid {
        "upstream.retry_metadata_invalid"
    } else {
        provider_code(class)
    };

    ProviderEvidence {
        class,
        code,
        retry_after_ms,
        reset_at_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW_MS: i64 = 1_700_000_000_000;

    #[test]
    fn acp_accepts_only_the_four_normative_token_paths() {
        for data in [
            json!({"code": "usage_limit_reached"}),
            json!({"type": "usage_limit_reached"}),
            json!({"error": {"code": "usage_limit_reached"}}),
            json!({"error": {"type": "usage_limit_reached"}}),
        ] {
            let evidence = classify_acp_error_data(false, Some(&data), NOW_MS);
            assert_eq!(evidence.class, DiagnosticFailureClass::ProviderLimit);
            assert_eq!(evidence.code, "upstream.provider_limit");
        }
        for data in [
            json!({"message": "usage_limit_reached"}),
            json!({"detail": {"code": "usage_limit_reached"}}),
            json!({"code": "Usage_Limit_Reached"}),
            json!({"code": 429}),
            json!("usage_limit_reached"),
        ] {
            let evidence = classify_acp_error_data(false, Some(&data), NOW_MS);
            assert_eq!(evidence.class, DiagnosticFailureClass::Unknown);
            assert_eq!(evidence.code, "upstream.unknown");
        }
    }

    #[test]
    fn acp_flat_nested_conflict_fails_closed() {
        let data = json!({
            "code": "usage_limit_reached",
            "error": {"type": "server_overloaded"}
        });
        let evidence = classify_acp_error_data(false, Some(&data), NOW_MS);
        assert_eq!(evidence.class, DiagnosticFailureClass::Unknown);
        assert_eq!(evidence.code, "upstream.classification_conflict");
    }

    #[test]
    fn acp_auth_required_is_authoritative() {
        let data = json!({
            "code": "usage_limit_reached",
            "type": "model_not_found"
        });
        let evidence = classify_acp_error_data(true, Some(&data), NOW_MS);
        assert_eq!(evidence.class, DiagnosticFailureClass::Authentication);
        assert_eq!(evidence.code, "upstream.authentication");
    }

    #[test]
    fn acp_structured_retry_and_reset_are_single_bounded_integers() {
        let data = json!({
            "code": "usage_limit_reached",
            "retry_after_ms": 1234,
            "reset_at_ms": NOW_MS + 5678
        });
        let evidence = classify_acp_error_data(false, Some(&data), NOW_MS);
        assert_eq!(evidence.retry_after_ms, Some(1234));
        assert_eq!(evidence.reset_at_ms, Some(NOW_MS + 5678));

        for data in [
            json!({"code": "usage_limit_reached", "retry_after_ms": -1}),
            json!({"code": "usage_limit_reached", "retry_after_ms": "1"}),
            json!({"code": "usage_limit_reached", "retry_after_ms": MAX_PROVIDER_RETRY_AFTER_MS + 1}),
            json!({"code": "usage_limit_reached", "reset_at_ms": -1}),
            json!({"code": "usage_limit_reached", "reset_at_ms": NOW_MS + MAX_PROVIDER_RETRY_AFTER_MS as i64 + 1}),
        ] {
            let evidence = classify_acp_error_data(false, Some(&data), NOW_MS);
            assert_eq!(evidence.class, DiagnosticFailureClass::ProviderLimit);
            assert_eq!(evidence.retry_after_ms, None);
            assert_eq!(evidence.reset_at_ms, None);
            assert_eq!(evidence.code, "upstream.retry_metadata_invalid");
        }
    }

    #[test]
    fn acp_duplicate_flat_and_nested_retry_values_are_invalid() {
        let data = json!({
            "code": "usage_limit_reached",
            "retry_after_ms": 1,
            "error": {"retry_after_ms": 1}
        });
        let evidence = classify_acp_error_data(false, Some(&data), NOW_MS);
        assert_eq!(evidence.class, DiagnosticFailureClass::ProviderLimit);
        assert_eq!(evidence.retry_after_ms, None);
        assert_eq!(evidence.code, "upstream.retry_metadata_invalid");
    }

    #[test]
    fn retry_metadata_never_promotes_unknown_acp_error() {
        let data = json!({"retry_after_ms": 1000});
        let evidence = classify_acp_error_data(false, Some(&data), NOW_MS);
        assert_eq!(evidence.class, DiagnosticFailureClass::Unknown);
        assert_eq!(evidence.retry_after_ms, Some(1000));
        assert_eq!(evidence.code, "upstream.unknown");
    }
}
