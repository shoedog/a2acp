//! Typed attested-prefix capability and transport status.
//!
//! This module intentionally contains no marker/prose parser. Marker recognition
//! belongs to the packaged ACP wrapper; bridge-core only carries typed evidence
//! and conservative fallback statuses.

use crate::error::BridgeError;
use crate::ids::TurnId;
use ring::rand::{SecureRandom, SystemRandom};

pub const ATTESTED_PREFIX_ISSUER_V1: &str = "bridge.acp.codex.commit-wrapper.v1";
pub const ATTESTED_PREFIX_META_KEY: &str = "dev.b2a.attested_prefix";
pub const ATTESTED_PREFIX_CAPABILITIES_METHOD: &str = "_b2a/apc-prefix/capabilities";
pub const ATTESTED_PREFIX_BEGIN_TURN_METHOD: &str = "_b2a/apc-prefix/beginTurn";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefixAttestationCapability {
    SupportedV1 {
        issuer_id: &'static str,
        boundary_scheme: PrefixBoundaryScheme,
    },
    Unsupported {
        reason: CapabilityUnavailableReason,
    },
}

impl<'de> serde::Deserialize<'de> for PrefixAttestationCapability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Wire {
            SupportedV1 {
                issuer_id: String,
                boundary_scheme: PrefixBoundaryScheme,
            },
            Unsupported {
                reason: CapabilityUnavailableReason,
            },
        }

        match <Wire as serde::Deserialize>::deserialize(deserializer)? {
            Wire::SupportedV1 {
                issuer_id,
                boundary_scheme,
            } => {
                if issuer_id != ATTESTED_PREFIX_ISSUER_V1 {
                    return Err(serde::de::Error::custom(
                        "unsupported prefix attestation issuer_id",
                    ));
                }
                Ok(Self::SupportedV1 {
                    issuer_id: ATTESTED_PREFIX_ISSUER_V1,
                    boundary_scheme,
                })
            }
            Wire::Unsupported { reason } => Ok(Self::Unsupported { reason }),
        }
    }
}

impl PrefixAttestationCapability {
    #[must_use]
    pub fn codex_commit_marker_v1() -> Self {
        Self::SupportedV1 {
            issuer_id: ATTESTED_PREFIX_ISSUER_V1,
            boundary_scheme: PrefixBoundaryScheme::CodexCommitMarkerV1,
        }
    }

    #[must_use]
    pub fn unsupported(reason: CapabilityUnavailableReason) -> Self {
        Self::Unsupported { reason }
    }

    #[must_use]
    pub fn is_supported_v1(&self) -> bool {
        matches!(
            self,
            Self::SupportedV1 {
                issuer_id,
                boundary_scheme: PrefixBoundaryScheme::CodexCommitMarkerV1,
            } if *issuer_id == ATTESTED_PREFIX_ISSUER_V1
        )
    }
}

impl Default for PrefixAttestationCapability {
    fn default() -> Self {
        Self::unsupported(CapabilityUnavailableReason::BackendDeclaredIncapable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefixBoundaryScheme {
    CodexCommitMarkerV1,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityUnavailableReason {
    BackendDeclaredIncapable,
    ProtocolDowngrade,
}

/// Node-scoped harvest sanitization mode (design §6: `"off"` /
/// `"attested_prefix_v1"`).
///
/// This is the §4.5 injection gate: every model-visible attested-prefix
/// surface (prompt-contract block, enabled `beginTurn`, wrapper marker
/// recognition) is reachable only through an enabled request, and an enabled
/// request is minted only for `AttestedPrefixV1` mode. Task P ships no
/// configuration surface for this mode — every production caller passes
/// `Off` — so with only Task P landed no model-visible behavior changes
/// (§15.1 acceptance criterion 16, §17 condition 5). Task F's per-node
/// `harvest_sanitization` TOML field becomes the only switch that can select
/// `AttestedPrefixV1`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarvestSanitizationMode {
    #[default]
    Off,
    AttestedPrefixV1,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PrefixAttestationRequest {
    #[default]
    Disabled,
    CodexCommitMarkerV1 {
        marker_nonce: [u8; 16],
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefixAttestationStatus {
    AttestedV1(AttestedPrefixV1),
    UnavailableV1(NoAttestationV1),
    Rejected(RejectedAttestation),
}

impl PrefixAttestationStatus {
    #[must_use]
    pub fn unavailable(
        producer_id: impl Into<String>,
        turn_id: impl Into<String>,
        reason: NoAttestationReason,
    ) -> Self {
        Self::UnavailableV1(NoAttestationV1 {
            producer_id: producer_id.into(),
            turn_id: turn_id.into(),
            reason,
        })
    }

    #[must_use]
    pub fn rejected(
        producer_id: impl Into<String>,
        turn_id: impl Into<String>,
        reason: InvalidAttestationReason,
    ) -> Self {
        Self::Rejected(RejectedAttestation {
            producer_id: producer_id.into(),
            turn_id: turn_id.into(),
            reason,
        })
    }
}

impl Default for PrefixAttestationStatus {
    fn default() -> Self {
        Self::unavailable("", "", NoAttestationReason::BackendDeclaredIncapable)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttestedPrefixV1 {
    pub issuer_id: String,
    pub producer_id: String,
    pub turn_id: String,
    pub body_len_bytes: u64,
    pub body_sha256: [u8; 32],
    pub process_prefix_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NoAttestationV1 {
    pub producer_id: String,
    pub turn_id: String,
    pub reason: NoAttestationReason,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RejectedAttestation {
    pub producer_id: String,
    pub turn_id: String,
    pub reason: InvalidAttestationReason,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoAttestationReason {
    BackendDeclaredIncapable,
    ProtocolDowngrade,
    SanitizationNotRequested,
    TurnMissingDeliverableBoundary,
    TurnEndedWithoutDeliverable,
    MultipleCommitMarkers,
    BackendProtocolViolation,
    BridgeSyntheticStreamError,
    BridgeSyntheticMissingDone,
    BridgeSyntheticCancellation,
    BridgeSyntheticEmptyFinal,
    BridgeSyntheticTwinDeath,
    BridgeStopReasonWithoutText,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidAttestationReason {
    UnsupportedVersion,
    MalformedMetadata,
    DuplicateControlMetadata,
    BackendCapabilityMismatch,
    UntrustedIssuer,
    ProducerMismatch,
    TurnMismatch,
    NonceMismatch,
    LengthMismatch,
    DigestMismatch,
    OffsetOverflow,
    OffsetOutOfBounds,
    EmptyDeliverable,
    OffsetNotUtf8Boundary,
}

#[must_use]
pub fn nonce_hex(nonce: &[u8; 16]) -> String {
    let mut out = String::with_capacity(32);
    for byte in nonce {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn generate_nonce() -> Result<[u8; 16], BridgeError> {
    let rng = SystemRandom::new();
    let mut bytes = [0_u8; 16];
    rng.fill(&mut bytes)
        .map_err(|_| BridgeError::ConfigInvalid {
            reason: "secure random nonce generation failed".into(),
        })?;
    Ok(bytes)
}

pub fn generate_turn_id() -> Result<TurnId, BridgeError> {
    let nonce = generate_nonce()?;
    TurnId::parse(format!("turn_{}", nonce_hex(&nonce)))
}

#[must_use]
pub fn commit_marker(nonce: &[u8; 16]) -> String {
    format!("<|b2a_apc_commit_v1:{}|>", nonce_hex(nonce))
}

#[must_use]
pub fn prompt_contract_block(nonce: &[u8; 16]) -> String {
    format!(
        "[B2A ATTESTED PREFIX PROTOCOL — REQUIRED]\n\n\
You may produce process or status text before the deliverable.\n\n\
Immediately before the first byte of the non-empty deliverable, emit the following\n\
ASCII control marker exactly once:\n\n\
{}\n\n\
Do not quote it, put it in a code fence, add whitespace to it, or add a newline\n\
between the marker and the first deliverable byte. The marker itself is not part\n\
of the deliverable.\n\n\
Do not emit the marker until a non-empty deliverable is ready. After emitting it,\n\
treat every later text byte as part of the deliverable.\n\n\
If your intended response must literally contain the exact marker, escape it with\n\
backslash parity: a literal marker uses one backslash immediately before it; a\n\
literal backslash plus a literal marker uses three. A commit marker preceded by\n\
one literal backslash uses two.\n\n\
If you cannot determine a valid non-empty deliverable boundary, do not emit the\n\
marker.",
        commit_marker(nonce)
    )
}

#[must_use]
pub fn append_prompt_contract(
    rendered_prompt: String,
    capability: &PrefixAttestationCapability,
    request: &PrefixAttestationRequest,
) -> String {
    if capability.is_supported_v1() {
        if let PrefixAttestationRequest::CodexCommitMarkerV1 { marker_nonce } = request {
            return format!(
                "{rendered_prompt}\n\n{}",
                prompt_contract_block(marker_nonce)
            );
        }
    }
    rendered_prompt
}

/// Mint the per-turn attestation request.
///
/// Enabled (`CodexCommitMarkerV1`) requires BOTH the §4.5 conditions that
/// exist at Task P time: node mode `attested_prefix_v1` AND a resolved
/// `SupportedV1` wrapper capability. `Off` mode always yields `Disabled`
/// regardless of capability, which in turn suppresses the prompt-contract
/// append, the private `beginTurn`, and wrapper marker recognition for the
/// turn. This function is the only production constructor of an enabled
/// request.
pub fn prefix_attestation_request_for_capability(
    mode: HarvestSanitizationMode,
    capability: &PrefixAttestationCapability,
) -> Result<PrefixAttestationRequest, BridgeError> {
    if mode == HarvestSanitizationMode::AttestedPrefixV1 && capability.is_supported_v1() {
        Ok(PrefixAttestationRequest::CodexCommitMarkerV1 {
            marker_nonce: generate_nonce()?,
        })
    } else {
        Ok(PrefixAttestationRequest::Disabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: [u8; 16] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];

    #[test]
    fn prompt_contract_block_matches_normative_golden() {
        let expected = "[B2A ATTESTED PREFIX PROTOCOL — REQUIRED]\n\n\
You may produce process or status text before the deliverable.\n\n\
Immediately before the first byte of the non-empty deliverable, emit the following\n\
ASCII control marker exactly once:\n\n\
<|b2a_apc_commit_v1:00112233445566778899aabbccddeeff|>\n\n\
Do not quote it, put it in a code fence, add whitespace to it, or add a newline\n\
between the marker and the first deliverable byte. The marker itself is not part\n\
of the deliverable.\n\n\
Do not emit the marker until a non-empty deliverable is ready. After emitting it,\n\
treat every later text byte as part of the deliverable.\n\n\
If your intended response must literally contain the exact marker, escape it with\n\
backslash parity: a literal marker uses one backslash immediately before it; a\n\
literal backslash plus a literal marker uses three. A commit marker preceded by\n\
one literal backslash uses two.\n\n\
If you cannot determine a valid non-empty deliverable boundary, do not emit the\n\
marker.";
        assert_eq!(prompt_contract_block(&NONCE), expected);
    }

    #[test]
    fn append_prompt_contract_appends_only_for_supported_enabled_request() {
        let prompt = "ordinary prompt".to_string();
        let appended = append_prompt_contract(
            prompt.clone(),
            &PrefixAttestationCapability::codex_commit_marker_v1(),
            &PrefixAttestationRequest::CodexCommitMarkerV1 {
                marker_nonce: NONCE,
            },
        );
        assert!(appended.starts_with("ordinary prompt\n\n[B2A ATTESTED PREFIX PROTOCOL"));
        assert!(appended.contains("<|b2a_apc_commit_v1:00112233445566778899aabbccddeeff|>"));

        assert_eq!(
            append_prompt_contract(
                prompt.clone(),
                &PrefixAttestationCapability::codex_commit_marker_v1(),
                &PrefixAttestationRequest::Disabled,
            ),
            prompt
        );
        assert_eq!(
            append_prompt_contract(
                "ordinary prompt".to_string(),
                &PrefixAttestationCapability::unsupported(
                    CapabilityUnavailableReason::BackendDeclaredIncapable,
                ),
                &PrefixAttestationRequest::CodexCommitMarkerV1 {
                    marker_nonce: NONCE,
                },
            ),
            "ordinary prompt"
        );
    }

    #[test]
    fn enabled_mode_request_matches_capability() {
        let supported = prefix_attestation_request_for_capability(
            HarvestSanitizationMode::AttestedPrefixV1,
            &PrefixAttestationCapability::codex_commit_marker_v1(),
        )
        .unwrap();
        assert!(matches!(
            supported,
            PrefixAttestationRequest::CodexCommitMarkerV1 { .. }
        ));
        assert_eq!(
            prefix_attestation_request_for_capability(
                HarvestSanitizationMode::AttestedPrefixV1,
                &PrefixAttestationCapability::unsupported(
                    CapabilityUnavailableReason::BackendDeclaredIncapable,
                ),
            )
            .unwrap(),
            PrefixAttestationRequest::Disabled
        );
    }

    #[test]
    fn off_mode_disables_request_even_for_supported_capability() {
        // §4.5 gate / §15.1 acceptance criterion 16: with sanitization OFF
        // (the only mode reachable while Task F's config is absent), a fully
        // capable wrapper backend still gets a Disabled request, so no prompt
        // contract is appended and no enabled beginTurn is sent.
        let request = prefix_attestation_request_for_capability(
            HarvestSanitizationMode::Off,
            &PrefixAttestationCapability::codex_commit_marker_v1(),
        )
        .unwrap();
        assert_eq!(request, PrefixAttestationRequest::Disabled);
        assert_eq!(
            append_prompt_contract(
                "ordinary prompt".to_string(),
                &PrefixAttestationCapability::codex_commit_marker_v1(),
                &request,
            ),
            "ordinary prompt"
        );
    }

    #[test]
    fn harvest_sanitization_mode_defaults_off_and_uses_spec_wire_names() {
        // §6: allowed values are exactly "off" and "attested_prefix_v1";
        // absent configuration means Off.
        assert_eq!(
            HarvestSanitizationMode::default(),
            HarvestSanitizationMode::Off
        );
        assert_eq!(
            serde_json::to_string(&HarvestSanitizationMode::Off).unwrap(),
            "\"off\""
        );
        assert_eq!(
            serde_json::to_string(&HarvestSanitizationMode::AttestedPrefixV1).unwrap(),
            "\"attested_prefix_v1\""
        );
    }

    #[test]
    fn generated_turn_id_uses_wire_format_turn_underscore_lower_hex() {
        let turn_id = generate_turn_id().expect("turn id generates");
        let raw = turn_id.as_str();
        assert_eq!(raw.len(), 37);
        assert!(raw.starts_with("turn_"));
        assert!(raw[5..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }
}
