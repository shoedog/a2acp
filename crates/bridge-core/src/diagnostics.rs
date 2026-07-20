//! Bounded, rollback-safe lifecycle diagnostics.
//!
//! The serialized DTOs in this module have private fields. Runtime strings enter
//! them only through builders that apply bridge-known credential redaction,
//! context-free sanitization, and schema bounds.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use tokio::sync::Mutex;

pub const DIAGNOSTIC_SCHEMA_V1: u16 = 1;
const MAX_CODE_BYTES: usize = 64;
const MAX_ID_BYTES: usize = 64;
const MAX_AUTH_METHODS: usize = 16;
const MAX_CAUSES: usize = 8;
const MAX_STDERR_LINES: usize = 32;
const MAX_TEXT_FIELD_BYTES: usize = 512;
const MAX_DIAGNOSTIC_TEXT_BYTES: usize = 8 * 1024;
const MAX_RETRY_AFTER_MS: u64 = 2_592_000_000;
const MAX_RESET_HORIZON_MS: i64 = 2_592_000_000;
const REDACTED_KNOWN_SECRET: &str = "[REDACTED KNOWN SECRET]";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticPhase {
    Resolve,
    Spawn,
    Initialize,
    Authenticate,
    SessionCreate,
    ConfigApply,
    PromptStart,
    PromptStream,
    PromptFinish,
    Teardown,
}

impl DiagnosticPhase {
    pub const ALL: [Self; 10] = [
        Self::Resolve,
        Self::Spawn,
        Self::Initialize,
        Self::Authenticate,
        Self::SessionCreate,
        Self::ConfigApply,
        Self::PromptStart,
        Self::PromptStream,
        Self::PromptFinish,
        Self::Teardown,
    ];

    fn is_pre_prompt(self) -> bool {
        matches!(
            self,
            Self::Resolve
                | Self::Spawn
                | Self::Initialize
                | Self::Authenticate
                | Self::SessionCreate
                | Self::ConfigApply
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    Started,
    Completed,
    Skipped,
    Failed,
}

impl PhaseStatus {
    pub const ALL: [Self; 4] = [Self::Started, Self::Completed, Self::Skipped, Self::Failed];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticFailureClass {
    Config,
    Authentication,
    Model,
    Protocol,
    Transport,
    AgentProcess,
    ContainerRuntime,
    ContainerImage,
    ContainerNetwork,
    ContainerMount,
    ContainerCredentials,
    Timeout,
    Overloaded,
    ProviderLimit,
    Persistence,
    Canceled,
    Unknown,
}

impl DiagnosticFailureClass {
    pub const ALL: [Self; 17] = [
        Self::Config,
        Self::Authentication,
        Self::Model,
        Self::Protocol,
        Self::Transport,
        Self::AgentProcess,
        Self::ContainerRuntime,
        Self::ContainerImage,
        Self::ContainerNetwork,
        Self::ContainerMount,
        Self::ContainerCredentials,
        Self::Timeout,
        Self::Overloaded,
        Self::ProviderLimit,
        Self::Persistence,
        Self::Canceled,
        Self::Unknown,
    ];

    pub fn is_container_fallback_class(self) -> bool {
        matches!(
            self,
            Self::ContainerRuntime
                | Self::ContainerImage
                | Self::ContainerNetwork
                | Self::ContainerMount
                | Self::ContainerCredentials
        )
    }

    fn allows_retry_same_target(self) -> bool {
        matches!(
            self,
            Self::Transport | Self::AgentProcess | Self::Timeout | Self::Overloaded
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureDisposition {
    Fatal,
    RetrySameTarget,
    ContainerFallbackCandidate,
}

impl FailureDisposition {
    pub const ALL: [Self; 3] = [
        Self::Fatal,
        Self::RetrySameTarget,
        Self::ContainerFallbackCandidate,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOperation {
    Mode,
    Model,
    Effort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StderrScope {
    Process,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StderrRedaction {
    BestEffort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DiagnosticBuildError {
    #[error("invalid diagnostic code")]
    InvalidCode,
    #[error("invalid diagnostic disposition")]
    InvalidDisposition,
    #[error("invalid stderr evidence")]
    InvalidStderrEvidence,
    #[error("invalid retry metadata")]
    InvalidRetryMetadata,
    #[error("invalid diagnostic event")]
    InvalidEvent,
    #[error("unsupported diagnostic schema")]
    UnsupportedSchema,
    #[error("invalid diagnostic observer capacity")]
    InvalidObserverCapacity,
}

/// Redacts only credential values already held by the bridge. Its `Debug`
/// implementation intentionally reports counts, never values.
#[derive(Clone)]
pub struct DiagnosticRedactor {
    known_values: Vec<String>,
    home_dir: Option<String>,
}

impl fmt::Debug for DiagnosticRedactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiagnosticRedactor")
            .field("known_value_count", &self.known_values.len())
            .field("has_home_dir", &self.home_dir.is_some())
            .finish()
    }
}

impl Default for DiagnosticRedactor {
    fn default() -> Self {
        Self::new(std::iter::empty::<String>())
    }
}

impl DiagnosticRedactor {
    pub fn new<I, S>(known_values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let known_values: Vec<String> = known_values
            .into_iter()
            .map(Into::into)
            .filter(|value| !value.is_empty())
            .collect();
        Self {
            known_values: Self::normalize_known_values(known_values),
            home_dir: std::env::var("HOME").ok().filter(|value| !value.is_empty()),
        }
    }

    /// Add exact bridge-known credential values while preserving the existing
    /// home-directory policy. Longer values remain first so an embedded shorter
    /// credential cannot partially rewrite a longer one before it is removed.
    #[must_use]
    pub fn with_known_values<I, S>(mut self, known_values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.known_values.extend(
            known_values
                .into_iter()
                .map(Into::into)
                .filter(|value| !value.is_empty()),
        );
        self.known_values = Self::normalize_known_values(self.known_values);
        self
    }

    fn normalize_known_values(mut known_values: Vec<String>) -> Vec<String> {
        known_values
            .sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        known_values.dedup();
        known_values
    }

    pub fn with_home_dir(mut self, home_dir: impl Into<String>) -> Self {
        let home_dir = home_dir.into();
        self.home_dir = (!home_dir.is_empty()).then_some(home_dir);
        self
    }

    /// Sanitize one process-stderr record before it enters an in-memory ring.
    ///
    /// This is intentionally the same second-line defense used by persisted
    /// diagnostics: exact bridge-known credential values, URL query/fragment
    /// data, secret markers, the operator home directory, and control
    /// characters are removed before the caller applies its storage bound.
    pub fn sanitize_stderr_line(&self, value: &str, max_bytes: usize) -> String {
        self.sanitize_text(value, max_bytes)
    }

    /// Sanitize one arbitrary identifier with the same all-or-nothing policy used by
    /// persisted authentication evidence. Callers receive either the complete safe id or
    /// a tagged redacted value; a partially rewritten identifier is never exposed.
    pub fn sanitize_diagnostic_id(&self, value: String) -> RedactedDiagnosticId {
        self.sanitize_id(value)
    }

    fn contains_known_value(&self, value: &str) -> bool {
        self.known_values.iter().any(|known| value.contains(known))
    }

    fn sanitize_text(&self, value: &str, max_bytes: usize) -> String {
        let mut sanitized: String = value
            .chars()
            .filter(|ch| *ch == '\t' || !ch.is_control())
            .collect();
        for known in &self.known_values {
            sanitized = sanitized.replace(known, REDACTED_KNOWN_SECRET);
        }
        sanitized = redact_url_query_and_fragment(sanitized);
        sanitized = redact_secret_markers(sanitized);
        sanitized = redact_credential_prefixes(sanitized);
        if let Some(home) = &self.home_dir {
            sanitized = sanitized.replace(home, "~");
        }
        truncate_utf8(&sanitized, max_bytes).to_owned()
    }

    fn sanitize_id(&self, value: String) -> RedactedDiagnosticId {
        if value.is_empty() || value.len() > MAX_ID_BYTES {
            return RedactedDiagnosticId::redacted();
        }
        let sanitized = self.sanitize_text(&value, MAX_ID_BYTES);
        if sanitized == value && !looks_secret_shaped(&value) {
            RedactedDiagnosticId::from_value(value)
        } else {
            RedactedDiagnosticId::redacted()
        }
    }

    fn adjacent_sensitive_indices(&self, values: &[String]) -> Vec<bool> {
        let mut concatenated = String::new();
        let mut spans = Vec::with_capacity(values.len());
        for value in values {
            let start = concatenated.len();
            concatenated.push_str(value);
            spans.push((start, concatenated.len()));
        }

        let mut sensitive = vec![false; values.len()];
        for known in &self.known_values {
            let mut search_from = 0;
            while search_from <= concatenated.len() {
                let Some(relative) = concatenated[search_from..].find(known) else {
                    break;
                };
                let start = search_from + relative;
                let end = start + known.len();
                for (index, (field_start, field_end)) in spans.iter().copied().enumerate() {
                    if start < field_end && end > field_start {
                        sensitive[index] = true;
                    }
                }
                search_from = end;
            }
        }
        sensitive
    }

    fn sanitize_collection(&self, values: Vec<String>) -> Vec<String> {
        let bounded: Vec<String> = values
            .into_iter()
            .map(|value| {
                let without_controls: String = value
                    .chars()
                    .filter(|ch| *ch == '\t' || !ch.is_control())
                    .collect();
                truncate_utf8(&without_controls, MAX_TEXT_FIELD_BYTES).to_owned()
            })
            .collect();
        let sensitive = self.adjacent_sensitive_indices(&bounded);
        bounded
            .into_iter()
            .zip(sensitive)
            .map(|(value, is_sensitive)| {
                if is_sensitive {
                    REDACTED_KNOWN_SECRET.to_owned()
                } else {
                    self.sanitize_text(&value, MAX_TEXT_FIELD_BYTES)
                }
            })
            .collect()
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn redact_url_query_and_fragment(mut value: String) -> String {
    let mut search_from = 0;
    loop {
        let lowercase = value.to_ascii_lowercase();
        let remaining = &lowercase[search_from..];
        let http = remaining.find("http://");
        let https = remaining.find("https://");
        let Some(relative_url_start) = (match (http, https) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        }) else {
            break;
        };
        let url_start = search_from + relative_url_start;
        let url_end = value[url_start..]
            .char_indices()
            .find_map(|(offset, ch)| {
                (offset > 0 && (ch.is_whitespace() || matches!(ch, ')' | ']' | '}' | '"' | '\'')))
                    .then_some(url_start + offset)
            })
            .unwrap_or(value.len());
        let secret_start = value[url_start..url_end]
            .find(['?', '#'])
            .map(|offset| url_start + offset);
        if let Some(secret_start) = secret_start {
            value.replace_range(secret_start..url_end, "[REDACTED URL]");
            search_from = secret_start + "[REDACTED URL]".len();
        } else {
            search_from = url_end;
        }
    }
    value
}

fn redact_secret_markers(value: String) -> String {
    const MARKERS: [&str; 8] = [
        "authorization",
        "access_token",
        "refresh_token",
        "set-cookie",
        "api_key",
        "bearer",
        "cookie",
        "token",
    ];
    let lowercase = value.to_ascii_lowercase();
    let mut best: Option<(usize, usize)> = None;
    for marker in MARKERS {
        let mut search_from = 0;
        while let Some(relative) = lowercase[search_from..].find(marker) {
            let start = search_from + relative;
            let end = start + marker.len();
            let identifier = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-');
            let left_ok = start == 0 || !identifier(lowercase.as_bytes()[start - 1]);
            let right_ok = end == lowercase.len() || !identifier(lowercase.as_bytes()[end]);
            if left_ok && right_ok && best.is_none_or(|(current, _)| start < current) {
                best = Some((start, end));
                break;
            }
            search_from = end;
        }
    }

    let Some((start, marker_end)) = best else {
        return value;
    };
    let mut value_start = marker_end;
    while value_start < value.len()
        && matches!(
            value.as_bytes()[value_start],
            b' ' | b'\t' | b':' | b'=' | b'"' | b'\''
        )
    {
        value_start += 1;
    }
    if value_start >= value.len() {
        return "[REDACTED LINE]".to_owned();
    }
    format!("{} [REDACTED]", value[..start].trim_end())
}

fn redact_credential_prefixes(mut value: String) -> String {
    const PREFIXES: [&str; 7] = [
        "sk-",
        "sk_",
        "ghp_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "xoxa-",
    ];
    const REDACTED: &str = "[REDACTED]";

    let mut search_from = 0;
    loop {
        let lowercase = value.to_ascii_lowercase();
        let mut next: Option<usize> = None;
        for prefix in PREFIXES {
            let mut prefix_search = search_from;
            while let Some(relative) = lowercase[prefix_search..].find(prefix) {
                let start = prefix_search + relative;
                let identifier =
                    |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-');
                if start == 0 || !identifier(lowercase.as_bytes()[start - 1]) {
                    next = Some(next.map_or(start, |current| current.min(start)));
                    break;
                }
                prefix_search = start + prefix.len();
            }
        }
        let Some(start) = next else {
            break;
        };
        let end = value[start..]
            .char_indices()
            .find_map(|(offset, ch)| {
                (offset > 0
                    && (ch.is_whitespace()
                        || matches!(
                            ch,
                            '"' | '\'' | '`' | '<' | '>' | ',' | ';' | ')' | ']' | '}'
                        )))
                .then_some(start + offset)
            })
            .unwrap_or(value.len());
        value.replace_range(start..end, REDACTED);
        search_from = start + REDACTED.len();
    }
    value
}

fn looks_secret_shaped(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if [
        "sk-",
        "sk_",
        "ghp_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "xoxa-",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
    {
        return true;
    }
    lower.split(['.', '_', '-']).any(|segment| {
        segment.len() >= 32
            && segment.bytes().all(|byte| byte.is_ascii_alphanumeric())
            && segment.bytes().any(|byte| byte.is_ascii_alphabetic())
            && segment.bytes().any(|byte| byte.is_ascii_digit())
    })
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DiagnosticCode(String);

impl DiagnosticCode {
    pub fn build(
        value: impl Into<String>,
        redactor: &DiagnosticRedactor,
    ) -> Result<Self, DiagnosticBuildError> {
        Self::build_inner(value.into(), Some(redactor))
    }

    fn build_static(value: String) -> Result<Self, DiagnosticBuildError> {
        Self::build_inner(value, None)
    }

    fn build_inner(
        value: String,
        redactor: Option<&DiagnosticRedactor>,
    ) -> Result<Self, DiagnosticBuildError> {
        if value.is_empty()
            || value.len() > MAX_CODE_BYTES
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            })
            || redactor.is_some_and(|redactor| redactor.contains_known_value(&value))
            || looks_secret_shaped(&value)
        {
            return Err(DiagnosticBuildError::InvalidCode);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DiagnosticCode").field(&self.0).finish()
    }
}

impl<'de> Deserialize<'de> for DiagnosticCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::build(value, &DiagnosticRedactor::default()).map_err(de::Error::custom)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RedactedDiagnosticId {
    state: RedactedDiagnosticIdState,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum RedactedDiagnosticIdState {
    Value { value: String },
    Redacted,
}

impl Serialize for RedactedDiagnosticId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.state.serialize(serializer)
    }
}

impl fmt::Debug for RedactedDiagnosticId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.state {
            RedactedDiagnosticIdState::Value { value } => {
                f.debug_tuple("Value").field(value).finish()
            }
            RedactedDiagnosticIdState::Redacted => f.write_str("Redacted"),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum RedactedDiagnosticIdWire {
    Value { value: String },
    Redacted,
}

impl<'de> Deserialize<'de> for RedactedDiagnosticId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match RedactedDiagnosticIdWire::deserialize(deserializer)? {
            RedactedDiagnosticIdWire::Value { value } => {
                DiagnosticRedactor::default().sanitize_id(value)
            }
            RedactedDiagnosticIdWire::Redacted => Self::redacted(),
        })
    }
}

impl RedactedDiagnosticId {
    fn from_value(value: String) -> Self {
        Self {
            state: RedactedDiagnosticIdState::Value { value },
        }
    }

    fn redacted() -> Self {
        Self {
            state: RedactedDiagnosticIdState::Redacted,
        }
    }

    fn as_value(&self) -> Option<&str> {
        match &self.state {
            RedactedDiagnosticIdState::Value { value } => Some(value),
            RedactedDiagnosticIdState::Redacted => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthenticationEvidenceInput {
    PreAuthenticated {
        advertised_method_ids: Vec<String>,
    },
    ConfiguredMethod {
        configured_id: String,
        advertised: bool,
    },
    SelectedAdvertisedMethod {
        selected_id: String,
    },
    NoMethodsAdvertised,
    ApiKeyEnv {
        name: String,
        present: bool,
    },
    NotApplicable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticationEvidence {
    kind: AuthenticationEvidenceKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AuthenticationEvidenceKind {
    PreAuthenticated {
        advertised_method_ids: Vec<RedactedDiagnosticId>,
        advertised_method_count: u16,
    },
    ConfiguredMethod {
        configured_id: RedactedDiagnosticId,
        advertised: bool,
    },
    SelectedAdvertisedMethod {
        selected_id: RedactedDiagnosticId,
    },
    NoMethodsAdvertised,
    ApiKeyEnv {
        name: RedactedDiagnosticId,
        present: bool,
    },
    NotApplicable,
}

impl Serialize for AuthenticationEvidence {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.kind.serialize(serializer)
    }
}

impl AuthenticationEvidence {
    pub fn build(input: AuthenticationEvidenceInput, redactor: &DiagnosticRedactor) -> Self {
        let kind = match input {
            AuthenticationEvidenceInput::PreAuthenticated {
                advertised_method_ids,
            } => {
                let advertised_method_count =
                    u16::try_from(advertised_method_ids.len()).unwrap_or(u16::MAX);
                let ids = build_redacted_ids(
                    advertised_method_ids
                        .into_iter()
                        .take(MAX_AUTH_METHODS)
                        .collect(),
                    redactor,
                );
                AuthenticationEvidenceKind::PreAuthenticated {
                    advertised_method_ids: ids,
                    advertised_method_count,
                }
            }
            AuthenticationEvidenceInput::ConfiguredMethod {
                configured_id,
                advertised,
            } => AuthenticationEvidenceKind::ConfiguredMethod {
                configured_id: redactor.sanitize_id(configured_id),
                advertised,
            },
            AuthenticationEvidenceInput::SelectedAdvertisedMethod { selected_id } => {
                AuthenticationEvidenceKind::SelectedAdvertisedMethod {
                    selected_id: redactor.sanitize_id(selected_id),
                }
            }
            AuthenticationEvidenceInput::NoMethodsAdvertised => {
                AuthenticationEvidenceKind::NoMethodsAdvertised
            }
            AuthenticationEvidenceInput::ApiKeyEnv { name, present } => {
                AuthenticationEvidenceKind::ApiKeyEnv {
                    name: redactor.sanitize_id(name),
                    present,
                }
            }
            AuthenticationEvidenceInput::NotApplicable => AuthenticationEvidenceKind::NotApplicable,
        };
        Self { kind }
    }

    fn dynamic_values(&self) -> Vec<String> {
        match &self.kind {
            AuthenticationEvidenceKind::PreAuthenticated {
                advertised_method_ids,
                ..
            } => advertised_method_ids
                .iter()
                .map(|id| id.as_value().unwrap_or_default().to_owned())
                .collect(),
            AuthenticationEvidenceKind::ConfiguredMethod { configured_id, .. } => {
                vec![configured_id.as_value().unwrap_or_default().to_owned()]
            }
            AuthenticationEvidenceKind::SelectedAdvertisedMethod { selected_id } => {
                vec![selected_id.as_value().unwrap_or_default().to_owned()]
            }
            AuthenticationEvidenceKind::ApiKeyEnv { name, .. } => {
                vec![name.as_value().unwrap_or_default().to_owned()]
            }
            AuthenticationEvidenceKind::NoMethodsAdvertised
            | AuthenticationEvidenceKind::NotApplicable => Vec::new(),
        }
    }

    fn apply_sensitive_indices(&mut self, sensitive: &[bool]) {
        let mut index = 0;
        let mut apply = |id: &mut RedactedDiagnosticId| {
            if sensitive.get(index).copied().unwrap_or(false) {
                *id = RedactedDiagnosticId::redacted();
            }
            index += 1;
        };
        match &mut self.kind {
            AuthenticationEvidenceKind::PreAuthenticated {
                advertised_method_ids,
                ..
            } => {
                for id in advertised_method_ids {
                    apply(id);
                }
            }
            AuthenticationEvidenceKind::ConfiguredMethod { configured_id, .. } => {
                apply(configured_id)
            }
            AuthenticationEvidenceKind::SelectedAdvertisedMethod { selected_id } => {
                apply(selected_id)
            }
            AuthenticationEvidenceKind::ApiKeyEnv { name, .. } => apply(name),
            AuthenticationEvidenceKind::NoMethodsAdvertised
            | AuthenticationEvidenceKind::NotApplicable => {}
        }
    }
}

fn build_redacted_ids(
    values: Vec<String>,
    redactor: &DiagnosticRedactor,
) -> Vec<RedactedDiagnosticId> {
    let mut ids: Vec<_> = values
        .into_iter()
        .map(|value| redactor.sanitize_id(value))
        .collect();
    let dynamic_values: Vec<String> = ids
        .iter()
        .map(|id| id.as_value().unwrap_or_default().to_owned())
        .collect();
    let sensitive = redactor.adjacent_sensitive_indices(&dynamic_values);
    for (id, sensitive) in ids.iter_mut().zip(sensitive) {
        if sensitive {
            *id = RedactedDiagnosticId::redacted();
        }
    }
    ids
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AuthenticationEvidenceWire {
    PreAuthenticated {
        advertised_method_ids: Vec<RedactedDiagnosticId>,
        advertised_method_count: u16,
    },
    ConfiguredMethod {
        configured_id: RedactedDiagnosticId,
        advertised: bool,
    },
    SelectedAdvertisedMethod {
        selected_id: RedactedDiagnosticId,
    },
    NoMethodsAdvertised,
    ApiKeyEnv {
        name: RedactedDiagnosticId,
        present: bool,
    },
    NotApplicable,
}

impl<'de> Deserialize<'de> for AuthenticationEvidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let kind = match AuthenticationEvidenceWire::deserialize(deserializer)? {
            AuthenticationEvidenceWire::PreAuthenticated {
                mut advertised_method_ids,
                advertised_method_count,
            } => {
                advertised_method_ids.truncate(MAX_AUTH_METHODS);
                AuthenticationEvidenceKind::PreAuthenticated {
                    advertised_method_ids,
                    advertised_method_count,
                }
            }
            AuthenticationEvidenceWire::ConfiguredMethod {
                configured_id,
                advertised,
            } => AuthenticationEvidenceKind::ConfiguredMethod {
                configured_id,
                advertised,
            },
            AuthenticationEvidenceWire::SelectedAdvertisedMethod { selected_id } => {
                AuthenticationEvidenceKind::SelectedAdvertisedMethod { selected_id }
            }
            AuthenticationEvidenceWire::NoMethodsAdvertised => {
                AuthenticationEvidenceKind::NoMethodsAdvertised
            }
            AuthenticationEvidenceWire::ApiKeyEnv { name, present } => {
                AuthenticationEvidenceKind::ApiKeyEnv { name, present }
            }
            AuthenticationEvidenceWire::NotApplicable => AuthenticationEvidenceKind::NotApplicable,
        };
        Ok(Self { kind })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhaseTransitionInput {
    pub phase: DiagnosticPhase,
    pub status: PhaseStatus,
    pub at_ms: i64,
    pub operation: Option<DiagnosticOperation>,
    pub code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseTransition {
    phase: DiagnosticPhase,
    status: PhaseStatus,
    at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operation: Option<DiagnosticOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<DiagnosticCode>,
}

impl PhaseTransition {
    pub fn build(
        input: PhaseTransitionInput,
        redactor: &DiagnosticRedactor,
    ) -> Result<Self, DiagnosticBuildError> {
        Ok(Self {
            phase: input.phase,
            status: input.status,
            at_ms: input.at_ms,
            operation: input.operation,
            code: input
                .code
                .map(|code| DiagnosticCode::build(code, redactor))
                .transpose()?,
        })
    }

    pub fn phase(&self) -> DiagnosticPhase {
        self.phase
    }

    pub fn status(&self) -> PhaseStatus {
        self.status
    }

    pub fn at_ms(&self) -> i64 {
        self.at_ms
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedPhaseTransitionInput {
    pub phase: DiagnosticPhase,
    pub status: PhaseStatus,
    pub at_ms: i64,
    pub operation: Option<DiagnosticOperation>,
    pub code: Option<String>,
    pub auth: Option<AuthenticationEvidenceInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedPhaseTransition {
    phase: DiagnosticPhase,
    status: PhaseStatus,
    at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operation: Option<DiagnosticOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<DiagnosticCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth: Option<AuthenticationEvidence>,
}

impl PersistedPhaseTransition {
    pub fn build(
        input: PersistedPhaseTransitionInput,
        redactor: &DiagnosticRedactor,
    ) -> Result<Self, DiagnosticBuildError> {
        Self::build_inner(input, redactor, false)
    }

    /// Build a transition whose code is a bridge-owned static token while all
    /// runtime auth evidence still uses the full known-value redactor. This
    /// prevents a short credential such as `a` from colliding with a trusted
    /// code without weakening redaction of dynamic fields.
    #[doc(hidden)]
    pub fn build_static_code(
        mut input: PersistedPhaseTransitionInput,
        static_code: Option<&'static str>,
        redactor: &DiagnosticRedactor,
    ) -> Result<Self, DiagnosticBuildError> {
        input.code = static_code.map(str::to_owned);
        Self::build_inner(input, redactor, true)
    }

    fn build_inner(
        input: PersistedPhaseTransitionInput,
        redactor: &DiagnosticRedactor,
        static_code: bool,
    ) -> Result<Self, DiagnosticBuildError> {
        let code = input
            .code
            .map(|code| {
                if static_code {
                    DiagnosticCode::build_static(code)
                } else {
                    DiagnosticCode::build(code, redactor)
                }
            })
            .transpose()?;
        let mut auth = input
            .auth
            .map(|auth| AuthenticationEvidence::build(auth, redactor));

        if let Some(auth) = auth.as_mut() {
            let mut dynamic = Vec::with_capacity(1 + auth.dynamic_values().len());
            dynamic.push(if static_code {
                String::new()
            } else {
                code.as_ref()
                    .map(|code| code.as_str().to_owned())
                    .unwrap_or_default()
            });
            dynamic.extend(auth.dynamic_values());
            let sensitive = redactor.adjacent_sensitive_indices(&dynamic);
            if sensitive.first().copied().unwrap_or(false) {
                return Err(DiagnosticBuildError::InvalidCode);
            }
            auth.apply_sensitive_indices(&sensitive[1..]);
        }

        Ok(Self {
            phase: input.phase,
            status: input.status,
            at_ms: input.at_ms,
            operation: input.operation,
            code,
            auth,
        })
    }

    pub fn phase(&self) -> DiagnosticPhase {
        self.phase
    }

    pub fn status(&self) -> PhaseStatus {
        self.status
    }

    pub fn at_ms(&self) -> i64 {
        self.at_ms
    }

    pub fn operation(&self) -> Option<DiagnosticOperation> {
        self.operation
    }

    pub fn code(&self) -> Option<&DiagnosticCode> {
        self.code.as_ref()
    }

    pub fn auth(&self) -> Option<&AuthenticationEvidence> {
        self.auth.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureDiagnosticInput {
    pub failed_phase: DiagnosticPhase,
    pub last_completed_phase: Option<DiagnosticPhase>,
    pub class: DiagnosticFailureClass,
    pub disposition: FailureDisposition,
    pub code: String,
    pub summary: String,
    pub causes: Vec<String>,
    pub stderr_observed: bool,
    pub stderr_line_count: u32,
    pub stderr_scope: Option<StderrScope>,
    pub stderr_tail: Option<Vec<String>>,
    pub stderr_redaction: Option<StderrRedaction>,
    pub retry_after_ms: Option<u64>,
    pub reset_at_ms: Option<i64>,
    pub prompt_may_have_been_accepted: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct FailureDiagnostic {
    schema_version: u16,
    failed_phase: DiagnosticPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_completed_phase: Option<DiagnosticPhase>,
    class: DiagnosticFailureClass,
    disposition: FailureDisposition,
    code: DiagnosticCode,
    summary: String,
    causes: Vec<String>,
    stderr_observed: bool,
    stderr_line_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stderr_scope: Option<StderrScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stderr_tail: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stderr_redaction: Option<StderrRedaction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reset_at_ms: Option<i64>,
    prompt_may_have_been_accepted: bool,
}

impl fmt::Debug for FailureDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FailureDiagnostic")
            .field("schema_version", &self.schema_version)
            .field("failed_phase", &self.failed_phase)
            .field("last_completed_phase", &self.last_completed_phase)
            .field("class", &self.class)
            .field("disposition", &self.disposition)
            .field("code", &self.code)
            .field("cause_count", &self.causes.len())
            .field("stderr_observed", &self.stderr_observed)
            .field("stderr_line_count", &self.stderr_line_count)
            .field("stderr_scope", &self.stderr_scope)
            .field("stderr_redaction", &self.stderr_redaction)
            .field("retry_after_ms", &self.retry_after_ms)
            .field("reset_at_ms", &self.reset_at_ms)
            .field(
                "prompt_may_have_been_accepted",
                &self.prompt_may_have_been_accepted,
            )
            .finish()
    }
}

impl FailureDiagnostic {
    pub fn build(
        input: FailureDiagnosticInput,
        redactor: &DiagnosticRedactor,
    ) -> Result<Self, DiagnosticBuildError> {
        Self::build_with_reference_time_inner(input, redactor, diagnostic_now_ms(), false)
    }

    /// Build a diagnostic whose code is a bridge-owned static token while all
    /// summaries, causes, and stderr retain the full known-value redactor.
    #[doc(hidden)]
    pub fn build_static_code(
        mut input: FailureDiagnosticInput,
        static_code: &'static str,
        redactor: &DiagnosticRedactor,
    ) -> Result<Self, DiagnosticBuildError> {
        input.code = static_code.to_owned();
        Self::build_with_reference_time_inner(input, redactor, diagnostic_now_ms(), true)
    }

    pub fn build_at(
        input: FailureDiagnosticInput,
        redactor: &DiagnosticRedactor,
        reference_time_ms: i64,
    ) -> Result<Self, DiagnosticBuildError> {
        Self::build_with_reference_time_inner(input, redactor, Some(reference_time_ms), false)
    }

    #[doc(hidden)]
    pub fn build_with_reference_time(
        input: FailureDiagnosticInput,
        redactor: &DiagnosticRedactor,
        reference_time_ms: Option<i64>,
    ) -> Result<Self, DiagnosticBuildError> {
        Self::build_with_reference_time_inner(input, redactor, reference_time_ms, false)
    }

    fn build_with_reference_time_inner(
        input: FailureDiagnosticInput,
        redactor: &DiagnosticRedactor,
        reference_time_ms: Option<i64>,
        static_code: bool,
    ) -> Result<Self, DiagnosticBuildError> {
        validate_disposition(&input)?;
        validate_stderr(&input)?;
        let reset_horizon = reference_time_ms
            .filter(|reference| *reference >= 0)
            .and_then(|reference| reference.checked_add(MAX_RESET_HORIZON_MS));
        if input
            .retry_after_ms
            .is_some_and(|value| value > MAX_RETRY_AFTER_MS)
            || input.reset_at_ms.is_some_and(|value| {
                value < 0 || reset_horizon.is_none_or(|horizon| value > horizon)
            })
        {
            return Err(DiagnosticBuildError::InvalidRetryMetadata);
        }

        let code = if static_code {
            DiagnosticCode::build_static(input.code)?
        } else {
            DiagnosticCode::build(input.code, redactor)?
        };
        let mut causes = if input.causes.len() > MAX_CAUSES {
            input
                .causes
                .iter()
                .take(2)
                .chain(input.causes.iter().skip(input.causes.len() - 6))
                .cloned()
                .collect()
        } else {
            input.causes
        };
        causes = redactor.sanitize_collection(causes);
        let mut summary = redactor.sanitize_text(&input.summary, MAX_TEXT_FIELD_BYTES);
        let mut stderr_tail = input.stderr_tail.map(|lines| {
            redactor.sanitize_collection(lines.into_iter().take(MAX_STDERR_LINES).collect())
        });

        let mut all_dynamic = Vec::new();
        all_dynamic.push(if static_code {
            String::new()
        } else {
            code.as_str().to_owned()
        });
        all_dynamic.push(summary.clone());
        all_dynamic.extend(causes.iter().cloned());
        if let Some(lines) = &stderr_tail {
            all_dynamic.extend(lines.iter().cloned());
        }
        let sensitive = redactor.adjacent_sensitive_indices(&all_dynamic);
        if sensitive.first().copied().unwrap_or(false) {
            return Err(DiagnosticBuildError::InvalidCode);
        }
        let mut index = 1;
        if sensitive.get(index).copied().unwrap_or(false) {
            summary = REDACTED_KNOWN_SECRET.to_owned();
        }
        index += 1;
        for cause in &mut causes {
            if sensitive.get(index).copied().unwrap_or(false) {
                *cause = REDACTED_KNOWN_SECRET.to_owned();
            }
            index += 1;
        }
        if let Some(lines) = stderr_tail.as_mut() {
            for line in lines {
                if sensitive.get(index).copied().unwrap_or(false) {
                    *line = REDACTED_KNOWN_SECRET.to_owned();
                }
                index += 1;
            }
        }

        enforce_text_budget(&mut summary, &mut causes, stderr_tail.as_mut());

        Ok(Self {
            schema_version: DIAGNOSTIC_SCHEMA_V1,
            failed_phase: input.failed_phase,
            last_completed_phase: input.last_completed_phase,
            class: input.class,
            disposition: input.disposition,
            code,
            summary,
            causes,
            stderr_observed: input.stderr_observed,
            stderr_line_count: input.stderr_line_count,
            stderr_scope: input.stderr_scope,
            stderr_tail,
            stderr_redaction: input.stderr_redaction,
            retry_after_ms: input.retry_after_ms,
            reset_at_ms: input.reset_at_ms,
            prompt_may_have_been_accepted: input.prompt_may_have_been_accepted,
        })
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub fn failed_phase(&self) -> DiagnosticPhase {
        self.failed_phase
    }

    pub fn last_completed_phase(&self) -> Option<DiagnosticPhase> {
        self.last_completed_phase
    }

    pub fn class(&self) -> DiagnosticFailureClass {
        self.class
    }

    pub fn disposition(&self) -> FailureDisposition {
        self.disposition
    }

    pub fn code(&self) -> &DiagnosticCode {
        &self.code
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn causes(&self) -> &[String] {
        &self.causes
    }

    pub fn stderr_tail(&self) -> Option<&[String]> {
        self.stderr_tail.as_deref()
    }

    pub fn prompt_may_have_been_accepted(&self) -> bool {
        self.prompt_may_have_been_accepted
    }

    /// Preserve the primary diagnostic while recording that bounded teardown also failed. The static
    /// marker is inserted first so the total diagnostic text budget cannot erase it behind opaque causes.
    #[doc(hidden)]
    pub fn with_secondary_teardown_marker(mut self) -> Self {
        const MARKER: &str = "teardown.secondary";
        self.causes.retain(|cause| cause != MARKER);
        if self.causes.len() == MAX_CAUSES {
            self.causes.pop();
        }
        self.causes.insert(0, MARKER.to_owned());
        enforce_text_budget(
            &mut self.summary,
            &mut self.causes,
            self.stderr_tail.as_mut(),
        );
        self
    }
}

fn validate_disposition(input: &FailureDiagnosticInput) -> Result<(), DiagnosticBuildError> {
    let barrier_is_consistent = match input.failed_phase {
        DiagnosticPhase::Resolve
        | DiagnosticPhase::Spawn
        | DiagnosticPhase::Initialize
        | DiagnosticPhase::Authenticate
        | DiagnosticPhase::SessionCreate
        | DiagnosticPhase::ConfigApply => !input.prompt_may_have_been_accepted,
        DiagnosticPhase::PromptStream | DiagnosticPhase::PromptFinish => {
            input.prompt_may_have_been_accepted
        }
        DiagnosticPhase::PromptStart | DiagnosticPhase::Teardown => true,
    };
    if !barrier_is_consistent {
        return Err(DiagnosticBuildError::InvalidDisposition);
    }

    match input.disposition {
        FailureDisposition::Fatal => Ok(()),
        FailureDisposition::RetrySameTarget
            if !input.prompt_may_have_been_accepted
                && input.failed_phase.is_pre_prompt()
                && input.class.allows_retry_same_target() =>
        {
            Ok(())
        }
        FailureDisposition::ContainerFallbackCandidate
            if !input.prompt_may_have_been_accepted
                && input.failed_phase.is_pre_prompt()
                && input.class.is_container_fallback_class() =>
        {
            Ok(())
        }
        FailureDisposition::RetrySameTarget | FailureDisposition::ContainerFallbackCandidate => {
            Err(DiagnosticBuildError::InvalidDisposition)
        }
    }
}

fn validate_stderr(input: &FailureDiagnosticInput) -> Result<(), DiagnosticBuildError> {
    if !input.stderr_observed {
        if input.stderr_line_count != 0
            || input.stderr_scope.is_some()
            || input.stderr_tail.is_some()
            || input.stderr_redaction.is_some()
        {
            return Err(DiagnosticBuildError::InvalidStderrEvidence);
        }
        return Ok(());
    }

    if input.stderr_line_count == 0 || input.stderr_scope != Some(StderrScope::Process) {
        return Err(DiagnosticBuildError::InvalidStderrEvidence);
    }
    if input.stderr_tail.as_ref().is_some_and(|lines| {
        u32::try_from(lines.len()).map_or(true, |len| len > input.stderr_line_count)
    }) {
        return Err(DiagnosticBuildError::InvalidStderrEvidence);
    }
    match (&input.stderr_tail, input.stderr_redaction) {
        (Some(_), Some(StderrRedaction::BestEffort)) | (None, None) => Ok(()),
        _ => Err(DiagnosticBuildError::InvalidStderrEvidence),
    }
}

fn diagnostic_now_ms() -> Option<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

fn enforce_text_budget(
    summary: &mut String,
    causes: &mut [String],
    stderr_tail: Option<&mut Vec<String>>,
) {
    let mut remaining = MAX_DIAGNOSTIC_TEXT_BYTES;
    let mut bound = |value: &mut String| {
        if value.len() > remaining {
            *value = truncate_utf8(value, remaining).to_owned();
        }
        remaining = remaining.saturating_sub(value.len());
    };
    bound(summary);
    for cause in causes {
        bound(cause);
    }
    if let Some(lines) = stderr_tail {
        for line in lines {
            bound(line);
        }
    }
}

#[derive(Deserialize)]
struct FailureDiagnosticWire {
    schema_version: u16,
    failed_phase: DiagnosticPhase,
    #[serde(default)]
    last_completed_phase: Option<DiagnosticPhase>,
    class: DiagnosticFailureClass,
    disposition: FailureDisposition,
    code: String,
    summary: String,
    #[serde(default)]
    causes: Vec<String>,
    stderr_observed: bool,
    stderr_line_count: u32,
    #[serde(default)]
    stderr_scope: Option<StderrScope>,
    #[serde(default)]
    stderr_tail: Option<Vec<String>>,
    #[serde(default)]
    stderr_redaction: Option<StderrRedaction>,
    #[serde(default)]
    retry_after_ms: Option<u64>,
    #[serde(default)]
    reset_at_ms: Option<i64>,
    prompt_may_have_been_accepted: bool,
}

impl<'de> Deserialize<'de> for FailureDiagnostic {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = FailureDiagnosticWire::deserialize(deserializer)?;
        if wire.schema_version != DIAGNOSTIC_SCHEMA_V1 {
            return Err(de::Error::custom(DiagnosticBuildError::UnsupportedSchema));
        }
        Self::build(
            FailureDiagnosticInput {
                failed_phase: wire.failed_phase,
                last_completed_phase: wire.last_completed_phase,
                class: wire.class,
                disposition: wire.disposition,
                code: wire.code,
                summary: wire.summary,
                causes: wire.causes,
                stderr_observed: wire.stderr_observed,
                stderr_line_count: wire.stderr_line_count,
                stderr_scope: wire.stderr_scope,
                stderr_tail: wire.stderr_tail,
                stderr_redaction: wire.stderr_redaction,
                retry_after_ms: wire.retry_after_ms,
                reset_at_ms: wire.reset_at_ms,
                prompt_may_have_been_accepted: wire.prompt_may_have_been_accepted,
            },
            &DiagnosticRedactor::default(),
        )
        .map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticEvent {
    transition: PersistedPhaseTransition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<FailureDiagnostic>,
}

impl DiagnosticEvent {
    pub fn new(
        transition: PersistedPhaseTransition,
        failure: Option<FailureDiagnostic>,
    ) -> Result<Self, DiagnosticBuildError> {
        if let Some(failure) = &failure {
            if transition.status != PhaseStatus::Failed
                || transition.phase != failure.failed_phase()
            {
                return Err(DiagnosticBuildError::InvalidEvent);
            }
        }
        Ok(Self {
            transition,
            failure,
        })
    }

    pub fn transition(&self) -> &PersistedPhaseTransition {
        &self.transition
    }

    pub fn failure(&self) -> Option<&FailureDiagnostic> {
        self.failure.as_ref()
    }
}

#[derive(Deserialize)]
struct DiagnosticEventWire {
    transition: PersistedPhaseTransition,
    #[serde(default)]
    failure: Option<FailureDiagnostic>,
}

impl<'de> Deserialize<'de> for DiagnosticEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = DiagnosticEventWire::deserialize(deserializer)?;
        Self::new(wire.transition, wire.failure).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OpenTransition {
    phase: DiagnosticPhase,
    operation: Option<DiagnosticOperation>,
}

#[derive(Clone, Default)]
struct DiagnosticSequence {
    open: Vec<OpenTransition>,
}

impl DiagnosticSequence {
    fn accept(&mut self, event: &DiagnosticEvent) -> Result<(), crate::error::BridgeError> {
        let transition = event.transition();
        let key = OpenTransition {
            phase: transition.phase(),
            operation: transition.operation(),
        };
        match transition.status() {
            PhaseStatus::Started => {
                if self.open.contains(&key) {
                    return Err(crate::error::BridgeError::InvalidStateTransition);
                }
                self.open.push(key);
            }
            PhaseStatus::Completed | PhaseStatus::Skipped | PhaseStatus::Failed => {
                let Some(index) = self.open.iter().rposition(|open| *open == key) else {
                    return Err(crate::error::BridgeError::InvalidStateTransition);
                };
                self.open.remove(index);
            }
        }
        Ok(())
    }
}

/// Diagnostic wall-clock timestamp for lifecycle transitions. Ordering remains
/// owned by journal sequence numbers; an unavailable wall clock degrades to the
/// Unix epoch rather than inventing a future time.
pub fn diagnostic_timestamp_ms() -> i64 {
    diagnostic_now_ms().unwrap_or(0)
}

/// Operation-scoped observer that validates transition grammar and discards
/// accepted events without persistence.
#[derive(Default)]
pub struct NoopDiagnosticObserver {
    sequence: Mutex<DiagnosticSequence>,
}

#[async_trait::async_trait]
impl crate::ports::DiagnosticObserver for NoopDiagnosticObserver {
    async fn record(&self, event: DiagnosticEvent) -> Result<(), crate::error::BridgeError> {
        self.sequence.lock().await.accept(&event)
    }
}

#[derive(Default)]
struct InMemoryDiagnosticState {
    sequence: DiagnosticSequence,
    events: VecDeque<DiagnosticEvent>,
    dropped: u64,
}

/// Bounded, non-durable operation observer for direct prompts and smoke.
pub struct InMemoryDiagnosticObserver {
    capacity: usize,
    include_redacted_stderr: bool,
    state: Mutex<InMemoryDiagnosticState>,
}

impl fmt::Debug for InMemoryDiagnosticObserver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemoryDiagnosticObserver")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl InMemoryDiagnosticObserver {
    pub fn new(capacity: usize) -> Result<Self, DiagnosticBuildError> {
        if capacity == 0 {
            return Err(DiagnosticBuildError::InvalidObserverCapacity);
        }
        Ok(Self {
            capacity,
            include_redacted_stderr: false,
            state: Mutex::new(InMemoryDiagnosticState::default()),
        })
    }

    #[must_use]
    pub fn with_redacted_stderr(mut self, include: bool) -> Self {
        self.include_redacted_stderr = include;
        self
    }

    pub async fn snapshot(&self) -> Vec<DiagnosticEvent> {
        self.state.lock().await.events.iter().cloned().collect()
    }

    pub async fn dropped_count(&self) -> u64 {
        self.state.lock().await.dropped
    }
}

#[async_trait::async_trait]
impl crate::ports::DiagnosticObserver for InMemoryDiagnosticObserver {
    fn include_redacted_stderr(&self) -> bool {
        self.include_redacted_stderr
    }

    async fn record(&self, event: DiagnosticEvent) -> Result<(), crate::error::BridgeError> {
        let mut state = self.state.lock().await;
        state.sequence.accept(&event)?;
        if state.events.len() == self.capacity {
            state.events.pop_front();
            state.dropped = state.dropped.saturating_add(1);
        }
        state.events.push_back(event);
        Ok(())
    }
}

/// Durable diagnostic observer. Construction proves the task row already
/// exists; every accepted transition is persisted before `record` returns.
pub struct TaskJournalDiagnosticObserver {
    store: Arc<dyn crate::task_store::TaskStore>,
    task: crate::ids::TaskId,
    operation: crate::ids::OperationId,
    sequence: Mutex<DiagnosticSequence>,
}

impl TaskJournalDiagnosticObserver {
    pub async fn new(
        store: Arc<dyn crate::task_store::TaskStore>,
        task: crate::ids::TaskId,
        operation: crate::ids::OperationId,
    ) -> Result<Self, crate::error::BridgeError> {
        if store.get(&task).await?.is_none() {
            return Err(crate::error::BridgeError::StoreFailure);
        }
        Ok(Self::from_verified_task(store, task, operation))
    }

    fn from_verified_task(
        store: Arc<dyn crate::task_store::TaskStore>,
        task: crate::ids::TaskId,
        operation: crate::ids::OperationId,
    ) -> Self {
        Self {
            store,
            task,
            operation,
            sequence: Mutex::new(DiagnosticSequence::default()),
        }
    }

    async fn commit_after_persist<F>(
        &self,
        event: &DiagnosticEvent,
        persist: F,
    ) -> Result<(), crate::error::BridgeError>
    where
        F: std::future::Future<Output = Result<(), crate::error::BridgeError>>,
    {
        let mut sequence = self.sequence.lock().await;
        let mut staged = sequence.clone();
        staged.accept(event)?;
        persist.await?;
        *sequence = staged;
        Ok(())
    }
}

#[async_trait::async_trait]
impl crate::ports::DiagnosticObserver for TaskJournalDiagnosticObserver {
    async fn record(&self, event: DiagnosticEvent) -> Result<(), crate::error::BridgeError> {
        let staged_event = event.clone();
        let at_ms = event.transition().at_ms();
        let store = self.store.clone();
        let task = self.task.clone();
        let operation = self.operation.clone();
        self.commit_after_persist(&staged_event, async move {
            store
                .record_event_sequenced(
                    &task,
                    &operation,
                    at_ms,
                    crate::orch::OrchEventKind::Progress {
                        progress: crate::orch::ProgressPayload::diagnostic(event),
                    },
                )
                .await?;
            Ok(())
        })
        .await
    }
}

#[derive(Default)]
pub struct NoopDiagnosticObserverFactory;

impl crate::ports::DiagnosticObserverFactory for NoopDiagnosticObserverFactory {
    fn make(
        &self,
        _node: &crate::ids::NodeId,
        _attempt: u32,
    ) -> Arc<dyn crate::ports::DiagnosticObserver> {
        Arc::new(NoopDiagnosticObserver::default())
    }
}

pub struct InMemoryDiagnosticObserverFactory {
    capacity: usize,
}

impl InMemoryDiagnosticObserverFactory {
    pub fn new(capacity: usize) -> Result<Self, DiagnosticBuildError> {
        InMemoryDiagnosticObserver::new(capacity)?;
        Ok(Self { capacity })
    }
}

impl crate::ports::DiagnosticObserverFactory for InMemoryDiagnosticObserverFactory {
    fn make(
        &self,
        _node: &crate::ids::NodeId,
        _attempt: u32,
    ) -> Arc<dyn crate::ports::DiagnosticObserver> {
        Arc::new(
            InMemoryDiagnosticObserver::new(self.capacity)
                .expect("factory capacity was validated at construction"),
        )
    }
}

pub struct TaskJournalDiagnosticObserverFactory {
    store: Arc<dyn crate::task_store::TaskStore>,
    task: crate::ids::TaskId,
    operation: crate::ids::OperationId,
}

impl TaskJournalDiagnosticObserverFactory {
    pub async fn new(
        store: Arc<dyn crate::task_store::TaskStore>,
        task: crate::ids::TaskId,
        operation: crate::ids::OperationId,
    ) -> Result<Self, crate::error::BridgeError> {
        if store.get(&task).await?.is_none() {
            return Err(crate::error::BridgeError::StoreFailure);
        }
        Ok(Self {
            store,
            task,
            operation,
        })
    }
}

impl crate::ports::DiagnosticObserverFactory for TaskJournalDiagnosticObserverFactory {
    fn make(
        &self,
        _node: &crate::ids::NodeId,
        _attempt: u32,
    ) -> Arc<dyn crate::ports::DiagnosticObserver> {
        Arc::new(TaskJournalDiagnosticObserver::from_verified_task(
            self.store.clone(),
            self.task.clone(),
            self.operation.clone(),
        ))
    }
}

#[cfg(test)]
mod observer_transaction_tests {
    use super::*;
    use crate::ids::{OperationId, TaskId};
    use crate::task_store::MemoryTaskStore;

    fn transition(status: PhaseStatus) -> DiagnosticEvent {
        DiagnosticEvent::new(
            PersistedPhaseTransition::build(
                PersistedPhaseTransitionInput {
                    phase: DiagnosticPhase::Resolve,
                    status,
                    at_ms: 1,
                    operation: None,
                    code: None,
                    auth: None,
                },
                &DiagnosticRedactor::default(),
            )
            .unwrap(),
            None,
        )
        .unwrap()
    }

    fn observer() -> Arc<TaskJournalDiagnosticObserver> {
        Arc::new(TaskJournalDiagnosticObserver::from_verified_task(
            Arc::new(MemoryTaskStore::new()),
            TaskId::parse("task-observer-transaction").unwrap(),
            OperationId::parse("op-observer-transaction").unwrap(),
        ))
    }

    #[tokio::test]
    async fn failed_persist_does_not_commit_transition_grammar() {
        let observer = observer();
        assert_eq!(
            observer
                .commit_after_persist(
                    &transition(PhaseStatus::Started),
                    std::future::ready(Err(crate::error::BridgeError::StoreFailure)),
                )
                .await,
            Err(crate::error::BridgeError::StoreFailure)
        );
        assert_eq!(
            observer
                .commit_after_persist(
                    &transition(PhaseStatus::Completed),
                    std::future::ready(Ok(())),
                )
                .await,
            Err(crate::error::BridgeError::InvalidStateTransition)
        );
    }

    #[tokio::test]
    async fn canceled_persist_does_not_commit_transition_grammar() {
        let observer = observer();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let pending_record = {
            let observer = observer.clone();
            let started = transition(PhaseStatus::Started);
            tokio::spawn(async move {
                observer
                    .commit_after_persist(&started, async move {
                        let _ = entered_tx.send(());
                        std::future::pending::<Result<(), crate::error::BridgeError>>().await
                    })
                    .await
            })
        };
        entered_rx.await.unwrap();
        pending_record.abort();
        assert!(pending_record.await.unwrap_err().is_cancelled());

        assert_eq!(
            observer
                .commit_after_persist(
                    &transition(PhaseStatus::Completed),
                    std::future::ready(Ok(())),
                )
                .await,
            Err(crate::error::BridgeError::InvalidStateTransition)
        );
    }
}
