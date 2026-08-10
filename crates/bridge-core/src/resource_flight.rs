//! R2f1b retained-resource flight contracts. Process generations are active through
//! `process::OwnedProcessTreeV1`; containers and remote providers remain adapters
//! for their separately gated slices.

use crate::ids::AttemptId;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

pub use crate::retained_resource_flight::{
    CleanupDeadlineTransferV1, FileResourceFlightJournal, InMemoryResourceFlightJournal,
    JournaledDispatchAdmissionV1, NodeCleanupAggregationV1, NoopResourceFlightResultPublisher,
    OwnedProcessTreeV1, ProcessBindingStageV1, ProcessSignalObservationV1, ResourceActionIntentV1,
    ResourceFlightJournal, ResourceFlightJournalError, ResourceFlightJournalEventV1,
    ResourceFlightJournalRecordV1, ResourceFlightKeyV1, ResourceFlightOwnerV1,
    ResourceFlightRegistryV1, ResourceFlightReservationOutcomeV1,
    ResourceFlightReservationRecordV1, ResourceFlightReservationV1, ResourceFlightResultPublisher,
    ResourceFlightTerminalAppendOutcomeV1, RetainedResourceFlight, RetainedResourceFlightConfigV1,
    RetainedResourceFlightError, RetainedResourceFlightGuardV1,
};

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
    Open {},
    AdmissionClosed {},
    IntentJournaled {},
    Signaling {},
    Settled { result: ResourceActionResultV1 },
}

/// R2f1b A2: `ResourceFlightIdV1` / `ResourceFlightStateV1` /
/// `ResourceIdentityV1` / `ProcessStartIdentityV1` (and their supporting
/// bounded types) type-contract coverage. These types are currently
/// signaled only through `execution_policy::NodeCleanupV2`'s `Pending` /
/// `Partial` / `Unknown` variants (never `ResourceFlightStateV1` itself, and
/// never `ResourceIdentityV1`); every construction here goes through the
/// same public constructors / struct literals a real caller would use.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{DiagnosticCode, DiagnosticFailureClass, DiagnosticRedactor};
    use crate::execution_policy::{BoundedCauseV1, Sha256HexV1};

    /// Deterministic 64-hex-digit `ResourceFlightIdV1`, mirroring the
    /// `flight_id()` helper in preparation_flight.rs's own test module.
    fn flight_id(digit: char) -> ResourceFlightIdV1 {
        ResourceFlightIdV1::parse(format!(
            "{}{}",
            ResourceFlightIdV1::PREFIX,
            digit.to_string().repeat(64)
        ))
        .unwrap()
    }

    fn sha(digit: char) -> Sha256HexV1 {
        Sha256HexV1::parse(digit.to_string().repeat(64)).unwrap()
    }

    fn sample_cause() -> BoundedCauseV1 {
        BoundedCauseV1 {
            failure_class: DiagnosticFailureClass::Persistence,
            code: DiagnosticCode::build("bridge.sample_cause", &DiagnosticRedactor::default())
                .unwrap(),
            deepest_cause: Some("boom".to_string()),
            cause_truncated: false,
            evidence_overflow: false,
            dependency_set: None,
        }
    }

    /// Discriminates: any change to the `#[serde(transparent)]` wire shape of
    /// `ResourceFlightIdV1` or its `resource-flight-` prefix.
    #[test]
    fn resource_flight_id_golden_wire_form_round_trips() {
        let id = flight_id('7');
        let golden = format!("\"resource-flight-{}\"", "7".repeat(64));
        assert_eq!(serde_json::to_string(&id).unwrap(), golden);
        let decoded: ResourceFlightIdV1 = serde_json::from_str(&golden).unwrap();
        assert_eq!(decoded, id);
    }

    /// Discriminates: `parse()` silently accepting a malformed id along any
    /// checked axis, including cross-accepting the *other* flight-id type's
    /// wire form (a `preparation-flight-` id must not parse as a
    /// `ResourceFlightIdV1` -- the two id spaces must stay disjoint since a
    /// later slice will use the prefix to route a raw persisted string).
    /// Mutation-checked: weakening the length check from `!=` to `<` left
    /// `long_by_one` green for the wrong reason; reverted before commit.
    #[test]
    fn resource_flight_id_parse_rejects_edges() {
        let expect_err = crate::error::BridgeError::InvalidRequest {
            field: "resource_flight_id",
        };
        let prefix = ResourceFlightIdV1::PREFIX;
        let cases: &[(&str, String)] = &[
            ("empty", String::new()),
            (
                "wrong_prefix_other_flight_type",
                format!("preparation-flight-{}", "1".repeat(64)),
            ),
            ("short_by_one", format!("{prefix}{}", "1".repeat(63))),
            ("long_by_one", format!("{prefix}{}", "1".repeat(65))),
            ("all_zero_suffix", format!("{prefix}{}", "0".repeat(64))),
            ("uppercase_hex", format!("{prefix}A{}", "1".repeat(63))),
            ("non_hex_char", format!("{prefix}g{}", "1".repeat(63))),
        ];
        for (name, input) in cases {
            assert_eq!(
                ResourceFlightIdV1::parse(input.clone()),
                Err(expect_err.clone()),
                "case {name} should be rejected"
            );
        }
        assert!(ResourceFlightIdV1::parse(format!("{prefix}{}", "1".repeat(64))).is_ok());
    }

    /// Discriminates: `mint()` producing a fixed/predictable id, or an id
    /// that fails its own `parse()`.
    #[test]
    fn resource_flight_id_mint_is_random_and_self_parseable() {
        let first = ResourceFlightIdV1::mint().unwrap();
        let second = ResourceFlightIdV1::mint().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.as_str().len(), ResourceFlightIdV1::ENCODED_LEN);
        assert_eq!(ResourceFlightIdV1::parse(first.as_str()).unwrap(), first);
    }

    /// Discriminates: the custom `Deserialize` impl accepting what `parse()`
    /// rejects.
    #[test]
    fn resource_flight_id_deserialize_rejects_what_parse_rejects() {
        assert!(serde_json::from_str::<ResourceFlightIdV1>("\"resource-flight-short\"").is_err());
        assert!(serde_json::from_str::<ResourceFlightIdV1>("123").is_err());
    }

    /// Discriminates: derive drift on `Ord`/`Hash` -- this id is collected
    /// into `BTreeSet`/`HashSet` in journaled cross-product checks
    /// downstream (see `execution_policy::NodeCleanupV2`).
    #[test]
    fn resource_flight_id_ord_and_hash_support_sorted_sets() {
        use std::collections::{BTreeSet, HashSet};
        let a = flight_id('1');
        let b = flight_id('2');
        assert!(a < b);
        let sorted: BTreeSet<_> = [b.clone(), a.clone()].into_iter().collect();
        assert_eq!(
            sorted.into_iter().collect::<Vec<_>>(),
            vec![a.clone(), b.clone()]
        );
        let mut set = HashSet::new();
        assert!(set.insert(a.clone()));
        assert!(!set.insert(a));
    }

    /// Discriminates: drift in `ProcessStartIdentityV1`'s field
    /// names/order/optionality, in both the `Some` and `None`
    /// `executable_sha256` shapes.
    #[test]
    fn process_start_identity_golden_round_trip_with_and_without_sha256() {
        let with_sha = ProcessStartIdentityV1 {
            pid: 4242,
            start_time_ticks: 100,
            executable_sha256: Some(sha('a')),
        };
        let golden_with = format!(
            "{{\"pid\":4242,\"start_time_ticks\":100,\"executable_sha256\":\"{}\"}}",
            "a".repeat(64)
        );
        assert_eq!(serde_json::to_string(&with_sha).unwrap(), golden_with);
        assert_eq!(
            serde_json::from_str::<ProcessStartIdentityV1>(&golden_with).unwrap(),
            with_sha
        );

        let without_sha = ProcessStartIdentityV1 {
            pid: 4242,
            start_time_ticks: 100,
            executable_sha256: None,
        };
        let golden_without = r#"{"pid":4242,"start_time_ticks":100,"executable_sha256":null}"#;
        assert_eq!(serde_json::to_string(&without_sha).unwrap(), golden_without);
        assert_eq!(
            serde_json::from_str::<ProcessStartIdentityV1>(golden_without).unwrap(),
            without_sha
        );
    }

    /// Positive control, contrasting the `BoundedCauseV1` finding elsewhere
    /// in this slice: `ProcessStartIdentityV1` DOES carry its own
    /// `#[serde(deny_unknown_fields)]`, so an unrecognized sibling key is
    /// genuinely rejected at this level.
    #[test]
    fn process_start_identity_rejects_unknown_field() {
        assert!(serde_json::from_str::<ProcessStartIdentityV1>(
            r#"{"pid":1,"start_time_ticks":1,"executable_sha256":null,"extra":1}"#
        )
        .is_err());
    }

    /// Discriminates: drift in the `AcpProcess` variant's tag/field
    /// shape/order, including its nested `ProcessStartIdentityV1` payload.
    #[test]
    fn resource_identity_acp_process_golden_round_trip() {
        let identity = ResourceIdentityV1::AcpProcess {
            generation: "gen-1".to_string(),
            spawn_nonce_sha256: sha('b'),
            pid: 100,
            pgid: Some(50),
            immutable_start: ProcessStartIdentityV1 {
                pid: 100,
                start_time_ticks: 7,
                executable_sha256: None,
            },
        };
        let golden = format!(
            "{{\"kind\":\"acp_process\",\"generation\":\"gen-1\",\"spawn_nonce_sha256\":\"{}\",\
             \"pid\":100,\"pgid\":50,\"immutable_start\":{{\"pid\":100,\"start_time_ticks\":7,\
             \"executable_sha256\":null}}}}",
            "b".repeat(64)
        );
        assert_eq!(serde_json::to_string(&identity).unwrap(), golden);
        assert_eq!(
            serde_json::from_str::<ResourceIdentityV1>(&golden).unwrap(),
            identity
        );
    }

    /// Discriminates: drift in the `ManagedContainer` variant's shape.
    #[test]
    fn resource_identity_managed_container_golden_round_trip() {
        let identity = ResourceIdentityV1::ManagedContainer {
            generation: "gen-2".to_string(),
            runtime: "docker".to_string(),
            immutable_container_id: "abc123".to_string(),
            ownership_labels_digest: sha('c'),
        };
        let golden = format!(
            "{{\"kind\":\"managed_container\",\"generation\":\"gen-2\",\"runtime\":\"docker\",\
             \"immutable_container_id\":\"abc123\",\"ownership_labels_digest\":\"{}\"}}",
            "c".repeat(64)
        );
        assert_eq!(serde_json::to_string(&identity).unwrap(), golden);
        assert_eq!(
            serde_json::from_str::<ResourceIdentityV1>(&golden).unwrap(),
            identity
        );
    }

    /// Discriminates: drift in the `DedicatedRemoteRequest` variant's shape
    /// (the sole payload-carrying single-field variant).
    #[test]
    fn resource_identity_dedicated_remote_request_golden_round_trip() {
        let identity = ResourceIdentityV1::DedicatedRemoteRequest {
            request_id: "req-1".to_string(),
        };
        let golden = r#"{"kind":"dedicated_remote_request","request_id":"req-1"}"#;
        assert_eq!(serde_json::to_string(&identity).unwrap(), golden);
        assert_eq!(
            serde_json::from_str::<ResourceIdentityV1>(golden).unwrap(),
            identity
        );
    }

    /// Discriminates: `#[serde(deny_unknown_fields)]` being dropped from
    /// `ResourceIdentityV1` itself.
    #[test]
    fn resource_identity_rejects_unknown_top_level_field() {
        assert!(serde_json::from_str::<ResourceIdentityV1>(
            r#"{"kind":"dedicated_remote_request","request_id":"req-1","extra":1}"#
        )
        .is_err());
    }

    /// Discriminates: an unrecognized `"kind"` tag value being silently
    /// accepted.
    #[test]
    fn resource_identity_rejects_unknown_variant_tag() {
        assert!(serde_json::from_str::<ResourceIdentityV1>(r#"{"kind":"bogus"}"#).is_err());
    }

    /// Discriminates: drift in any of `ResourceActionDispositionV1`'s five
    /// externally-tagged (bare-string) variant names.
    #[test]
    fn resource_action_disposition_all_variants_golden_round_trip() {
        let cases = [
            (ResourceActionDispositionV1::Complete, "\"complete\""),
            (ResourceActionDispositionV1::Partial, "\"partial\""),
            (ResourceActionDispositionV1::Failed, "\"failed\""),
            (ResourceActionDispositionV1::Unknown, "\"unknown\""),
            (ResourceActionDispositionV1::NotNeeded, "\"not_needed\""),
        ];
        for (variant, golden) in cases {
            assert_eq!(serde_json::to_string(&variant).unwrap(), golden);
            assert_eq!(
                serde_json::from_str::<ResourceActionDispositionV1>(golden).unwrap(),
                variant
            );
        }
    }

    /// Discriminates: any change to the pinned wire form of a plain
    /// (already canonical) recovery reason.
    #[test]
    fn bounded_recovery_reason_golden_round_trip() {
        let reason = BoundedRecoveryReasonV1::new("crash").unwrap();
        assert_eq!(serde_json::to_string(&reason).unwrap(), "\"crash\"");
        let decoded: BoundedRecoveryReasonV1 = serde_json::from_str("\"crash\"").unwrap();
        assert_eq!(decoded, reason);
    }

    /// Discriminates: `new()` accepting a value that sanitizes down to empty.
    #[test]
    fn bounded_recovery_reason_rejects_empty_after_sanitize() {
        let expect_err = crate::error::BridgeError::InvalidRequest {
            field: "recovery_reason",
        };
        assert_eq!(BoundedRecoveryReasonV1::new(""), Err(expect_err.clone()));
        assert_eq!(BoundedRecoveryReasonV1::new("\u{7}"), Err(expect_err));
    }

    /// Discriminates: `new()` not applying the `MAX_RECOVERY_REASON_BYTES` bound.
    #[test]
    fn bounded_recovery_reason_truncates_at_max_bytes() {
        let long = "a".repeat(MAX_RECOVERY_REASON_BYTES + 50);
        let reason = BoundedRecoveryReasonV1::new(&long).unwrap();
        assert_eq!(reason.as_str().len(), MAX_RECOVERY_REASON_BYTES);
        assert_eq!(reason.as_str(), "a".repeat(MAX_RECOVERY_REASON_BYTES));
    }

    /// Discriminates: the custom `Deserialize` impl accepting a raw string
    /// that sanitization would rewrite, instead of requiring the
    /// already-canonical form.
    #[test]
    fn bounded_recovery_reason_deserialize_rejects_noncanonical_input() {
        let raw_with_control_byte = "a\u{7}b";
        let encoded = serde_json::to_string(raw_with_control_byte).unwrap();
        assert!(serde_json::from_str::<BoundedRecoveryReasonV1>(&encoded).is_err());
        assert!(serde_json::from_str::<BoundedRecoveryReasonV1>("\"ab\"").is_ok());
    }

    /// Discriminates: derive drift on `Ord`/`Hash` for the reason newtype.
    #[test]
    fn bounded_recovery_reason_ord_and_hash_support_sorted_sets() {
        use std::collections::BTreeSet;
        let a = BoundedRecoveryReasonV1::new("a").unwrap();
        let b = BoundedRecoveryReasonV1::new("b").unwrap();
        assert!(a < b);
        let sorted: BTreeSet<_> = [b.clone(), a.clone()].into_iter().collect();
        assert_eq!(sorted.into_iter().collect::<Vec<_>>(), vec![a, b]);
    }

    /// Discriminates: drift in `RecoveryOwnerV1`'s field shape/order, and a
    /// dropped `#[serde(deny_unknown_fields)]` on it (positive control: all
    /// three of its fields are transparent-string newtypes, so unlike
    /// `BoundedCauseV1` there is no nested object for an unknown field to
    /// hide inside).
    #[test]
    fn recovery_owner_golden_round_trip_and_rejects_unknown_field() {
        let owner = RecoveryOwnerV1 {
            attempt_id: crate::ids::AttemptId::parse(format!("attempt-{}", "1".repeat(32)))
                .unwrap(),
            resource_flight_id: flight_id('2'),
            reason: BoundedRecoveryReasonV1::new("crash").unwrap(),
        };
        let golden = format!(
            "{{\"attempt_id\":\"attempt-{}\",\"resource_flight_id\":\"resource-flight-{}\",\
             \"reason\":\"crash\"}}",
            "1".repeat(32),
            "2".repeat(64)
        );
        assert_eq!(serde_json::to_string(&owner).unwrap(), golden);
        assert_eq!(
            serde_json::from_str::<RecoveryOwnerV1>(&golden).unwrap(),
            owner
        );

        let with_extra = golden.replacen('}', ",\"extra\":1}", 1);
        assert!(serde_json::from_str::<RecoveryOwnerV1>(&with_extra).is_err());
    }

    /// Discriminates: drift in `ResourceActionResultV1`'s field shape/order,
    /// across both the fully-populated and fully-absent-optional shapes.
    #[test]
    fn resource_action_result_golden_round_trip_populated_and_empty() {
        let owner = RecoveryOwnerV1 {
            attempt_id: crate::ids::AttemptId::parse(format!("attempt-{}", "3".repeat(32)))
                .unwrap(),
            resource_flight_id: flight_id('4'),
            reason: BoundedRecoveryReasonV1::new("crash").unwrap(),
        };
        let populated = ResourceActionResultV1 {
            disposition: ResourceActionDispositionV1::Partial,
            duration_ms: 500,
            recovery_owner: Some(owner.clone()),
            cause: Some(sample_cause()),
        };
        let golden_populated = format!(
            "{{\"disposition\":\"partial\",\"duration_ms\":500,\"recovery_owner\":{{\
             \"attempt_id\":\"attempt-{}\",\"resource_flight_id\":\"resource-flight-{}\",\
             \"reason\":\"crash\"}},\"cause\":{{\"failure_class\":\"persistence\",\
             \"code\":\"bridge.sample_cause\",\"deepest_cause\":\"boom\",\
             \"cause_truncated\":false,\"evidence_overflow\":false,\"dependency_set\":null}}}}",
            "3".repeat(32),
            "4".repeat(64)
        );
        assert_eq!(serde_json::to_string(&populated).unwrap(), golden_populated);
        assert_eq!(
            serde_json::from_str::<ResourceActionResultV1>(&golden_populated).unwrap(),
            populated
        );

        let empty = ResourceActionResultV1 {
            disposition: ResourceActionDispositionV1::Complete,
            duration_ms: 10,
            recovery_owner: None,
            cause: None,
        };
        let golden_empty =
            r#"{"disposition":"complete","duration_ms":10,"recovery_owner":null,"cause":null}"#;
        assert_eq!(serde_json::to_string(&empty).unwrap(), golden_empty);
        assert_eq!(
            serde_json::from_str::<ResourceActionResultV1>(golden_empty).unwrap(),
            empty
        );
    }

    /// Discriminates: `#[serde(deny_unknown_fields)]` being dropped from
    /// `ResourceActionResultV1` itself (positive control at this struct's
    /// own level, contrasting the nested-`cause` finding below).
    #[test]
    fn resource_action_result_rejects_unknown_top_level_field() {
        assert!(serde_json::from_str::<ResourceActionResultV1>(
            r#"{"disposition":"complete","duration_ms":10,"recovery_owner":null,"cause":null,"extra":1}"#
        )
        .is_err());
    }

    /// R2f1b slice-2a tightening (was the A2 FINDING test, which pinned the
    /// gap by asserting `is_ok()`): `ResourceActionResultV1` denied unknown
    /// fields at its own level, but its nested `cause: Option<BoundedCauseV1>`
    /// did not, so an unrecognized key inside `cause` was silently accepted
    /// via `ResourceFlightStateV1::Settled { result }` -- the exact path slice
    /// 3 will journal, and one with no canonical re-encode check to catch it.
    /// `NodeCauseV1` now carries its own `#[serde(deny_unknown_fields)]` (see
    /// `execution_policy::r2f1b_node_cause_strictness_tests`), closing the gap
    /// before the path gains writers. Discriminates: that attribute being
    /// dropped again; the positive control below pins that the well-formed
    /// nested cause still decodes.
    #[test]
    fn resource_flight_state_settled_cause_rejects_unknown_nested_field() {
        let json = r#"{"state":"settled","result":{"disposition":"failed","duration_ms":1,"recovery_owner":null,"cause":{"failure_class":"persistence","code":"bridge.sample_cause","deepest_cause":"boom","cause_truncated":false,"evidence_overflow":false,"dependency_set":null,"unexpected_extra_field":"leaked"}}}"#;
        assert!(serde_json::from_str::<ResourceFlightStateV1>(json).is_err());

        let without_extra = json.replace(",\"unexpected_extra_field\":\"leaked\"", "");
        assert!(serde_json::from_str::<ResourceFlightStateV1>(&without_extra).is_ok());
    }

    /// Discriminates: drift in the internally-tagged wire shape of the four
    /// unit variants.
    #[test]
    fn resource_flight_state_unit_variants_golden_round_trip() {
        let cases = [
            (ResourceFlightStateV1::Open {}, r#"{"state":"open"}"#),
            (
                ResourceFlightStateV1::AdmissionClosed {},
                r#"{"state":"admission_closed"}"#,
            ),
            (
                ResourceFlightStateV1::IntentJournaled {},
                r#"{"state":"intent_journaled"}"#,
            ),
            (
                ResourceFlightStateV1::Signaling {},
                r#"{"state":"signaling"}"#,
            ),
        ];
        for (variant, golden) in cases {
            assert_eq!(serde_json::to_string(&variant).unwrap(), golden);
            assert_eq!(
                serde_json::from_str::<ResourceFlightStateV1>(golden).unwrap(),
                variant
            );
        }
    }

    /// Discriminates: drift in the `Settled { result }` payload shape.
    #[test]
    fn resource_flight_state_settled_golden_round_trip() {
        let state = ResourceFlightStateV1::Settled {
            result: ResourceActionResultV1 {
                disposition: ResourceActionDispositionV1::Complete,
                duration_ms: 10,
                recovery_owner: None,
                cause: None,
            },
        };
        let golden = r#"{"state":"settled","result":{"disposition":"complete","duration_ms":10,"recovery_owner":null,"cause":null}}"#;
        assert_eq!(serde_json::to_string(&state).unwrap(), golden);
        assert_eq!(
            serde_json::from_str::<ResourceFlightStateV1>(golden).unwrap(),
            state
        );
    }

    /// Discriminates: `#[serde(deny_unknown_fields)]` being dropped from
    /// `ResourceFlightStateV1` for its struct-payload variant (`Settled`).
    /// See the test immediately below, which covers the same guarantee for
    /// the (as of this slice) empty-struct variant `Open`.
    #[test]
    fn resource_flight_state_rejects_unknown_field_on_struct_variant() {
        assert!(serde_json::from_str::<ResourceFlightStateV1>(
            r#"{"state":"settled","result":{"disposition":"complete","duration_ms":10,"recovery_owner":null,"cause":null},"extra":1}"#
        )
        .is_err());
    }

    /// R2f1b F-2 tightening: same root cause as the matching test in
    /// preparation_flight.rs -- `#[serde(tag = "state", deny_unknown_fields)]`
    /// on an internally-tagged enum only enforces `deny_unknown_fields` for
    /// struct-payload variants; a bare unit variant like `Open`,
    /// `AdmissionClosed`, `IntentJournaled`, or `Signaling` used to
    /// deserialize without ever checking for leftover keys in the buffered
    /// JSON object. `Open`, `AdmissionClosed`, `IntentJournaled`, and
    /// `Signaling` are now declared as empty-struct payloads (`Open {}`)
    /// instead of unit variants: the wire form is byte-identical
    /// (`{"state":"open"}`, see the golden round-trip test above), but the
    /// derive now buffers the variant's fields like any other struct
    /// payload, so a stray sibling key is rejected here too. This closes
    /// the gap the FINDING test used to pin.
    #[test]
    fn resource_flight_state_rejects_unknown_field_on_unit_variant() {
        assert!(
            serde_json::from_str::<ResourceFlightStateV1>(r#"{"state":"open","extra":1}"#).is_err()
        );
    }

    /// Discriminates: an unrecognized `"state"` tag value being silently
    /// accepted.
    #[test]
    fn resource_flight_state_rejects_unknown_variant_tag() {
        assert!(serde_json::from_str::<ResourceFlightStateV1>(r#"{"state":"bogus"}"#).is_err());
    }
}
