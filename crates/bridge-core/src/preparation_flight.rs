//! Inactive R2f1b preparation-flight contracts and clock plumbing.
//!
//! Preparation flights bound pre-effect filesystem/custody work. This slice defines the
//! shared monotonic-clock plumbing only; later slices own scheduling and cancellation.

use crate::attempt_activity::MonotonicClock;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const MAX_PREPARATION_TRANSFER_REASON_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PreparationFlightIdV1(String);

impl PreparationFlightIdV1 {
    pub const PREFIX: &'static str = "preparation-flight-";
    pub const ENCODED_LEN: usize = Self::PREFIX.len() + 64;

    pub fn mint() -> Result<Self, crate::error::BridgeError> {
        let mut bytes = [0_u8; 32];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| crate::error::BridgeError::IdentityUnavailable)?;
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
                    field: "preparation_flight_id",
                })?;
        if value.len() != Self::ENCODED_LEN
            || suffix.bytes().all(|byte| byte == b'0')
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(crate::error::BridgeError::InvalidRequest {
                field: "preparation_flight_id",
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PreparationFlightIdV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct BoundedPreparationTransferReasonV1(String);

impl BoundedPreparationTransferReasonV1 {
    pub fn new(value: impl AsRef<str>) -> Result<Self, crate::error::BridgeError> {
        let sanitized = crate::diagnostics::DiagnosticRedactor::default()
            .sanitize_stderr_line(value.as_ref(), MAX_PREPARATION_TRANSFER_REASON_BYTES);
        if sanitized.is_empty() {
            return Err(crate::error::BridgeError::InvalidRequest {
                field: "preparation_transfer_reason",
            });
        }
        Ok(Self(sanitized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for BoundedPreparationTransferReasonV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let parsed = Self::new(&value).map_err(serde::de::Error::custom)?;
        if parsed.as_str() != value {
            return Err(serde::de::Error::custom(
                "non-canonical preparation transfer reason",
            ));
        }
        Ok(parsed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreparationFlightStateV1 {
    Open,
    BarrierSynced,
    Transferred {
        reason: BoundedPreparationTransferReasonV1,
    },
    Failed {
        cause: crate::execution_policy::BoundedCauseV1,
    },
}

#[derive(Clone)]
pub struct PreparationClockV1 {
    clock: Arc<dyn MonotonicClock>,
}

impl std::fmt::Debug for PreparationClockV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PreparationClockV1 { .. }")
    }
}

impl PreparationClockV1 {
    #[must_use]
    pub fn new(clock: Arc<dyn MonotonicClock>) -> Self {
        Self { clock }
    }

    #[must_use]
    pub fn elapsed_ms(&self) -> u64 {
        self.clock.elapsed_ms()
    }
}
