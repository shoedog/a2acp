use bridge_core::diagnostics::DiagnosticFailureClass;
use bridge_core::provider::{
    provider_code, provider_token_class, ProviderEvidence, MAX_PROVIDER_RETRY_AFTER_MS,
};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::StatusCode;
use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::fmt;
use std::time::UNIX_EPOCH;

pub(crate) const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;
pub(crate) const MAX_RETRY_AFTER_MS: u64 = MAX_PROVIDER_RETRY_AFTER_MS;

#[derive(Clone, Debug, Default)]
enum Hint<T> {
    #[default]
    Missing,
    Valid(T),
    Invalid,
}

#[derive(Default)]
struct ParsedError {
    tokens: Vec<String>,
    classification_ambiguous: bool,
    retry_after_ms: Hint<u64>,
    reset_at_ms: Hint<i64>,
}

impl<'de> Deserialize<'de> for ParsedError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ParsedErrorVisitor;

        impl<'de> Visitor<'de> for ParsedErrorVisitor {
            type Value = ParsedError;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an OpenAI-compatible error object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut parsed = ParsedError::default();
                let mut code_seen = false;
                let mut type_seen = false;
                let mut retry_seen = false;
                let mut reset_seen = false;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "code" | "type" => {
                            let seen = if key == "code" {
                                &mut code_seen
                            } else {
                                &mut type_seen
                            };
                            let value = map.next_value::<Value>()?;
                            if *seen {
                                parsed.classification_ambiguous = true;
                            } else if let Value::String(token) = value {
                                parsed.tokens.push(token);
                            }
                            *seen = true;
                        }
                        "retry_after_ms" => {
                            let value = map.next_value::<Value>()?;
                            parsed.retry_after_ms = if retry_seen {
                                Hint::Invalid
                            } else {
                                parse_retry_delta(value)
                            };
                            retry_seen = true;
                        }
                        "reset_at_ms" => {
                            let value = map.next_value::<Value>()?;
                            parsed.reset_at_ms = if reset_seen {
                                Hint::Invalid
                            } else {
                                parse_reset_timestamp(value)
                            };
                            reset_seen = true;
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(parsed)
            }
        }

        deserializer.deserialize_map(ParsedErrorVisitor)
    }
}

#[derive(Default)]
struct ParsedEnvelope {
    error: Option<ParsedError>,
}

impl<'de> Deserialize<'de> for ParsedEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct EnvelopeVisitor;

        impl<'de> Visitor<'de> for EnvelopeVisitor {
            type Value = ParsedEnvelope;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object containing one error object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut error = None;
                let mut error_seen = false;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "error" {
                        if error_seen {
                            return Err(de::Error::duplicate_field("error"));
                        }
                        error = Some(map.next_value::<ParsedError>()?);
                        error_seen = true;
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(ParsedEnvelope { error })
            }
        }

        deserializer.deserialize_map(EnvelopeVisitor)
    }
}

fn parse_retry_delta(value: Value) -> Hint<u64> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .filter(|value| *value <= MAX_RETRY_AFTER_MS)
            .map_or(Hint::Invalid, Hint::Valid),
        _ => Hint::Invalid,
    }
}

fn parse_reset_timestamp(value: Value) -> Hint<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .filter(|value| *value >= 0)
            .map_or(Hint::Invalid, Hint::Valid),
        _ => Hint::Invalid,
    }
}

fn status_is_compatible(status: StatusCode, class: DiagnosticFailureClass) -> bool {
    match class {
        DiagnosticFailureClass::ProviderLimit => {
            matches!(status.as_u16(), 402 | 429)
        }
        DiagnosticFailureClass::Overloaded => matches!(status.as_u16(), 429 | 503 | 529),
        DiagnosticFailureClass::Authentication => matches!(status.as_u16(), 401 | 403),
        DiagnosticFailureClass::Model => matches!(status.as_u16(), 400 | 404),
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
        | DiagnosticFailureClass::Unknown => false,
    }
}

enum HeaderHint {
    Missing,
    RetryAfter(u64),
    ResetAt(i64),
    Invalid,
}

fn parse_retry_after(headers: &HeaderMap, now_ms: i64) -> HeaderHint {
    let mut values = headers.get_all(RETRY_AFTER).iter();
    let Some(value) = values.next() else {
        return HeaderHint::Missing;
    };
    if values.next().is_some() {
        return HeaderHint::Invalid;
    }
    let Ok(value) = value.to_str() else {
        return HeaderHint::Invalid;
    };

    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .ok()
            .filter(|seconds| *seconds <= MAX_RETRY_AFTER_MS / 1000)
            .and_then(|seconds| seconds.checked_mul(1000))
            .map_or(HeaderHint::Invalid, HeaderHint::RetryAfter);
    }

    let Ok(parsed) = httpdate::parse_http_date(value) else {
        return HeaderHint::Invalid;
    };
    if httpdate::fmt_http_date(parsed) != value {
        return HeaderHint::Invalid;
    }
    let Ok(since_epoch) = parsed.duration_since(UNIX_EPOCH) else {
        return HeaderHint::Invalid;
    };
    let Ok(reset_at_ms) = i64::try_from(since_epoch.as_millis()) else {
        return HeaderHint::Invalid;
    };
    let Some(horizon) = now_ms.checked_add(MAX_RETRY_AFTER_MS as i64) else {
        return HeaderHint::Invalid;
    };
    if reset_at_ms > horizon {
        HeaderHint::Invalid
    } else {
        HeaderHint::ResetAt(reset_at_ms)
    }
}

pub(crate) fn classify_http_error(
    status: StatusCode,
    body: &[u8],
    body_oversized: bool,
    headers: &HeaderMap,
    now_ms: i64,
) -> ProviderEvidence {
    let parsed = if body_oversized {
        ParsedError::default()
    } else {
        serde_json::from_slice::<ParsedEnvelope>(body)
            .ok()
            .and_then(|envelope| envelope.error)
            .unwrap_or_default()
    };

    let (class, classification_conflict) = if matches!(status.as_u16(), 401 | 403) {
        (DiagnosticFailureClass::Authentication, false)
    } else if parsed.classification_ambiguous {
        (DiagnosticFailureClass::Unknown, false)
    } else {
        let mut recognized = parsed
            .tokens
            .iter()
            .filter_map(|token| provider_token_class(token));
        match recognized.next() {
            None => (DiagnosticFailureClass::Unknown, false),
            Some(first) => {
                let conflict = recognized.any(|class| class != first);
                if conflict {
                    (DiagnosticFailureClass::Unknown, true)
                } else if status_is_compatible(status, first) {
                    (first, false)
                } else {
                    (DiagnosticFailureClass::Unknown, false)
                }
            }
        }
    };

    let horizon = now_ms.checked_add(MAX_RETRY_AFTER_MS as i64);
    let (mut retry_after_ms, mut metadata_invalid) = match parsed.retry_after_ms {
        Hint::Missing => (None, false),
        Hint::Valid(value) => (Some(value), false),
        Hint::Invalid => (None, true),
    };
    let (mut reset_at_ms, reset_invalid) = match parsed.reset_at_ms {
        Hint::Missing => (None, false),
        Hint::Valid(value) if horizon.is_some_and(|horizon| value <= horizon) => {
            (Some(value), false)
        }
        Hint::Valid(_) | Hint::Invalid => (None, true),
    };
    metadata_invalid |= reset_invalid;

    let mut metadata_conflict = false;
    match parse_retry_after(headers, now_ms) {
        HeaderHint::Missing => {}
        HeaderHint::Invalid => metadata_invalid = true,
        HeaderHint::RetryAfter(value) => match retry_after_ms {
            Some(structured) if structured != value => metadata_conflict = true,
            Some(_) => {}
            None if !matches!(parsed.retry_after_ms, Hint::Invalid) => {
                retry_after_ms = Some(value);
            }
            None => {}
        },
        HeaderHint::ResetAt(value) => match reset_at_ms {
            Some(structured) if structured != value => metadata_conflict = true,
            Some(_) => {}
            None if !matches!(parsed.reset_at_ms, Hint::Invalid | Hint::Valid(_)) => {
                reset_at_ms = Some(value);
            }
            None => {}
        },
    }

    let code = if classification_conflict {
        "upstream.classification_conflict"
    } else if metadata_conflict {
        "upstream.retry_metadata_conflict"
    } else if metadata_invalid {
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
    use reqwest::header::{HeaderName, HeaderValue, RETRY_AFTER};
    use std::time::{Duration, UNIX_EPOCH};

    const NOW_MS: i64 = 1_700_000_000_000;

    fn classify(status: u16, body: &str) -> ProviderEvidence {
        classify_with_headers(status, body.as_bytes(), false, &[])
    }

    fn classify_with_headers(
        status: u16,
        body: &[u8],
        oversized: bool,
        headers: &[(&str, &str)],
    ) -> ProviderEvidence {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        classify_http_error(
            StatusCode::from_u16(status).unwrap(),
            body,
            oversized,
            &map,
            NOW_MS,
        )
    }

    #[test]
    fn every_recognized_token_requires_its_compatible_http_status() {
        let cases = [
            (
                "insufficient_quota",
                402,
                DiagnosticFailureClass::ProviderLimit,
            ),
            ("quota_exceeded", 429, DiagnosticFailureClass::ProviderLimit),
            (
                "billing_hard_limit_reached",
                402,
                DiagnosticFailureClass::ProviderLimit,
            ),
            (
                "usage_limit_reached",
                429,
                DiagnosticFailureClass::ProviderLimit,
            ),
            (
                "usage_limit_exceeded",
                429,
                DiagnosticFailureClass::ProviderLimit,
            ),
            (
                "rate_limit_exceeded",
                429,
                DiagnosticFailureClass::ProviderLimit,
            ),
            (
                "rate_limit_error",
                429,
                DiagnosticFailureClass::ProviderLimit,
            ),
            ("overloaded_error", 429, DiagnosticFailureClass::Overloaded),
            ("server_overloaded", 503, DiagnosticFailureClass::Overloaded),
            ("capacity_exceeded", 529, DiagnosticFailureClass::Overloaded),
            (
                "temporarily_unavailable",
                503,
                DiagnosticFailureClass::Overloaded,
            ),
            (
                "authentication_error",
                401,
                DiagnosticFailureClass::Authentication,
            ),
            (
                "invalid_api_key",
                403,
                DiagnosticFailureClass::Authentication,
            ),
            (
                "permission_error",
                403,
                DiagnosticFailureClass::Authentication,
            ),
            ("model_not_found", 404, DiagnosticFailureClass::Model),
            ("invalid_model", 400, DiagnosticFailureClass::Model),
            ("unsupported_model", 400, DiagnosticFailureClass::Model),
        ];

        for (token, status, expected) in cases {
            for field in ["code", "type"] {
                let body = format!(r#"{{"error":{{"{field}":"{token}"}}}}"#);
                assert_eq!(classify(status, &body).class, expected, "{field}={token}");
                assert_eq!(
                    classify(500, &body).class,
                    DiagnosticFailureClass::Unknown,
                    "incompatible status accepted {field}={token}"
                );
            }
        }
    }

    #[test]
    fn auth_status_is_authoritative_but_other_bare_capacity_statuses_are_unknown() {
        for status in [401, 403] {
            let failure = classify(
                status,
                r#"{"error":{"code":"usage_limit_reached","type":"model_not_found"}}"#,
            );
            assert_eq!(failure.class, DiagnosticFailureClass::Authentication);
            assert_eq!(failure.code, "upstream.authentication");
        }
        for status in [429, 503, 529] {
            let failure = classify(status, "");
            assert_eq!(failure.class, DiagnosticFailureClass::Unknown);
            assert_eq!(failure.code, "upstream.unknown");
        }
    }

    #[test]
    fn conflicting_recognized_fields_fail_closed() {
        let failure = classify(
            429,
            r#"{"error":{"code":"usage_limit_reached","type":"overloaded_error"}}"#,
        );
        assert_eq!(failure.class, DiagnosticFailureClass::Unknown);
        assert_eq!(failure.code, "upstream.classification_conflict");
    }

    #[test]
    fn malformed_fuzzy_wrong_case_and_wrong_shape_bodies_are_unknown() {
        let cases = [
            r#"{"error":{"code":"Usage_Limit_Reached"}}"#,
            r#"{"error":{"code":"prefix_usage_limit_reached_suffix"}}"#,
            r#"{"error":{"code":429}}"#,
            r#"{"error":{"code":{"value":"usage_limit_reached"}}}"#,
            r#"[{"error":{"code":"usage_limit_reached"}}]"#,
            r#"{"error":{"code":"usage_limit_reached"}} trailing"#,
            r#"{"error":{"code":"usage_limit_reached"}"#,
        ];
        for body in cases {
            assert_eq!(
                classify(429, body).class,
                DiagnosticFailureClass::Unknown,
                "accepted {body}"
            );
        }
    }

    #[test]
    fn error_body_boundary_is_exact_and_oversized_body_has_no_evidence() {
        let prefix = r#"{"error":{"code":"usage_limit_reached"}}"#;
        let mut exact = prefix.as_bytes().to_vec();
        exact.resize(MAX_ERROR_BODY_BYTES, b' ');
        assert_eq!(
            classify_with_headers(429, &exact, false, &[]).class,
            DiagnosticFailureClass::ProviderLimit
        );
        exact.push(b' ');
        assert_eq!(
            classify_with_headers(429, &exact[..MAX_ERROR_BODY_BYTES], true, &[]).class,
            DiagnosticFailureClass::Unknown
        );
    }

    #[test]
    fn structured_retry_metadata_is_bounded_integer_only() {
        let valid = classify(
            429,
            &format!(
                r#"{{"error":{{"code":"usage_limit_reached","retry_after_ms":{},"reset_at_ms":{}}}}}"#,
                MAX_RETRY_AFTER_MS,
                NOW_MS + MAX_RETRY_AFTER_MS as i64
            ),
        );
        assert_eq!(valid.retry_after_ms, Some(MAX_RETRY_AFTER_MS));
        assert_eq!(valid.reset_at_ms, Some(NOW_MS + MAX_RETRY_AFTER_MS as i64));
        assert_eq!(valid.code, "upstream.provider_limit");

        for body in [
            r#"{"error":{"code":"usage_limit_reached","retry_after_ms":-1}}"#,
            r#"{"error":{"code":"usage_limit_reached","retry_after_ms":1e3}}"#,
            r#"{"error":{"code":"usage_limit_reached","retry_after_ms":"1000"}}"#,
            r#"{"error":{"code":"usage_limit_reached","retry_after_ms":2592000001}}"#,
            r#"{"error":{"code":"usage_limit_reached","retry_after_ms":1,"retry_after_ms":2}}"#,
        ] {
            let failure = classify(429, body);
            assert_eq!(failure.class, DiagnosticFailureClass::ProviderLimit);
            assert_eq!(failure.retry_after_ms, None, "accepted {body}");
            assert_eq!(failure.code, "upstream.retry_metadata_invalid");
        }
    }

    #[test]
    fn single_retry_after_fills_only_the_matching_missing_hint() {
        let decimal = classify_with_headers(
            429,
            br#"{"error":{"code":"usage_limit_reached"}}"#,
            false,
            &[("retry-after", "17")],
        );
        assert_eq!(decimal.retry_after_ms, Some(17_000));
        assert_eq!(decimal.reset_at_ms, None);

        let reset_ms = NOW_MS + 60_000;
        let date = httpdate::fmt_http_date(
            UNIX_EPOCH + Duration::from_millis(u64::try_from(reset_ms).unwrap()),
        );
        let dated = classify_with_headers(
            429,
            br#"{"error":{"code":"usage_limit_reached"}}"#,
            false,
            &[("retry-after", &date)],
        );
        assert_eq!(dated.retry_after_ms, None);
        assert_eq!(dated.reset_at_ms, Some(reset_ms));
    }

    #[test]
    fn duplicate_malformed_and_out_of_range_retry_after_are_omitted() {
        let cases = [
            vec![("retry-after", "1"), ("retry-after", "2")],
            vec![("retry-after", "+1")],
            vec![("retry-after", "1.5")],
            vec![("retry-after", "2592001")],
            vec![("retry-after", "not-a-date")],
        ];
        for headers in cases {
            let failure = classify_with_headers(
                429,
                br#"{"error":{"code":"usage_limit_reached"}}"#,
                false,
                &headers,
            );
            assert_eq!(failure.retry_after_ms, None);
            assert_eq!(failure.reset_at_ms, None);
            assert_eq!(failure.code, "upstream.retry_metadata_invalid");
        }
    }

    #[test]
    fn structured_hint_wins_and_conflicting_header_is_reported() {
        let same = classify_with_headers(
            429,
            br#"{"error":{"code":"usage_limit_reached","retry_after_ms":1000}}"#,
            false,
            &[("retry-after", "1")],
        );
        assert_eq!(same.retry_after_ms, Some(1000));
        assert_eq!(same.code, "upstream.provider_limit");

        let conflict = classify_with_headers(
            429,
            br#"{"error":{"code":"usage_limit_reached","retry_after_ms":1000}}"#,
            false,
            &[("retry-after", "2")],
        );
        assert_eq!(conflict.retry_after_ms, Some(1000));
        assert_eq!(conflict.code, "upstream.retry_metadata_conflict");
    }

    #[test]
    fn retry_metadata_never_changes_classification() {
        let failure = classify_with_headers(
            500,
            br#"{"error":{"retry_after_ms":1000}}"#,
            false,
            &[(RETRY_AFTER.as_str(), "2")],
        );
        assert_eq!(failure.class, DiagnosticFailureClass::Unknown);
        assert_eq!(failure.retry_after_ms, Some(1000));
        assert_eq!(failure.code, "upstream.retry_metadata_conflict");
    }
}
