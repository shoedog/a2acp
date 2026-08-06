//! Inactive R2f1b retained-resource flight contracts.
//!
//! These types describe the durable ownership boundary used by later R2f1b slices. This
//! slice deliberately does not signal processes, containers, or providers.

use crate::ids::AttemptId;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

pub const MAX_RECOVERY_REASON_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ResourceFlightIdV1(String);

impl ResourceFlightIdV1 {
    pub const PREFIX: &'static str = "resource-flight-";
    pub const ENCODED_LEN: usize = Self::PREFIX.len() + 64;

    pub fn mint() -> Result<Self, crate::error::BridgeError> {
        let mut bytes = [0_u8; 32];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| crate::error::BridgeError::IdentityUnavailable)?;
        Self::from_bytes(bytes)
    }

    fn from_bytes(bytes: [u8; 32]) -> Result<Self, crate::error::BridgeError> {
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(crate::error::BridgeError::IdentityUnavailable);
        }
        let mut value = String::with_capacity(Self::ENCODED_LEN);
        value.push_str(Self::PREFIX);
        for byte in bytes {
            use std::fmt::Write as _;
            let _ = write!(&mut value, "{byte:02x}");
        }
        Ok(Self(value))
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, crate::error::BridgeError> {
        let value = value.into();
        let suffix =
            value
                .strip_prefix(Self::PREFIX)
                .ok_or(crate::error::BridgeError::InvalidRequest {
                    field: "resource_flight_id",
                })?;
        if value.len() != Self::ENCODED_LEN
            || suffix.bytes().all(|byte| byte == b'0')
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(crate::error::BridgeError::InvalidRequest {
                field: "resource_flight_id",
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ResourceFlightIdV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessStartIdentityV1 {
    pub pid: u32,
    pub start_time_ticks: u64,
    pub executable_sha256: Option<crate::execution_policy::Sha256HexV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceIdentityV1 {
    AcpProcess {
        generation: String,
        spawn_nonce_sha256: crate::execution_policy::Sha256HexV1,
        pid: u32,
        pgid: Option<u32>,
        immutable_start: ProcessStartIdentityV1,
    },
    ManagedContainer {
        generation: String,
        runtime: String,
        immutable_container_id: String,
        ownership_labels_digest: crate::execution_policy::Sha256HexV1,
    },
    DedicatedRemoteRequest {
        request_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceActionDispositionV1 {
    Complete,
    Partial,
    Failed,
    Unknown,
    NotNeeded,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct BoundedRecoveryReasonV1(String);

impl BoundedRecoveryReasonV1 {
    pub fn new(value: impl AsRef<str>) -> Result<Self, crate::error::BridgeError> {
        let sanitized = crate::diagnostics::DiagnosticRedactor::default()
            .sanitize_stderr_line(value.as_ref(), MAX_RECOVERY_REASON_BYTES);
        if sanitized.is_empty() {
            return Err(crate::error::BridgeError::InvalidRequest {
                field: "recovery_reason",
            });
        }
        Ok(Self(sanitized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for BoundedRecoveryReasonV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let parsed = Self::new(&value).map_err(serde::de::Error::custom)?;
        if parsed.as_str() != value {
            return Err(serde::de::Error::custom("non-canonical recovery reason"));
        }
        Ok(parsed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryOwnerV1 {
    pub attempt_id: AttemptId,
    pub resource_flight_id: ResourceFlightIdV1,
    pub reason: BoundedRecoveryReasonV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceActionResultV1 {
    pub disposition: ResourceActionDispositionV1,
    pub duration_ms: u64,
    pub recovery_owner: Option<RecoveryOwnerV1>,
    pub cause: Option<crate::execution_policy::BoundedCauseV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceFlightStateV1 {
    Open,
    AdmissionClosed,
    IntentJournaled,
    Signaling,
    Settled { result: ResourceActionResultV1 },
}
