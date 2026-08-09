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
    Open {},
    BarrierSynced {},
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

/// R2f1b A2: `PreparationFlightIdV1` / `BoundedPreparationTransferReasonV1` /
/// `PreparationFlightStateV1` / `PreparationClockV1` type-contract coverage.
/// These types are currently inert (no other module constructs or persists
/// them yet), so every construction here goes through the same public
/// constructors a future caller would use -- no private-field backdoor.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{DiagnosticCode, DiagnosticFailureClass, DiagnosticRedactor};
    use crate::execution_policy::BoundedCauseV1;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Deterministic 64-hex-digit `PreparationFlightIdV1` for controllable
    /// sort-key / equality tests: real `mint()` output is effectively
    /// unordered, so a hand-picked repeated-digit suffix lets a test pin
    /// exactly which id sorts first (mirrors the `fp()`/`cid()` helper
    /// pattern used for `WorktreeCustodyIdV1` in execution_policy.rs).
    fn flight_id(digit: char) -> PreparationFlightIdV1 {
        PreparationFlightIdV1::parse(format!(
            "{}{}",
            PreparationFlightIdV1::PREFIX,
            digit.to_string().repeat(64)
        ))
        .unwrap()
    }

    /// A representative `BoundedCauseV1` (== `NodeCauseV1`) built entirely
    /// through its `pub` fields / `DiagnosticCode::build`, for exercising
    /// `PreparationFlightStateV1::Failed { cause }`.
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
    /// `PreparationFlightIdV1` (e.g. accidentally wrapping it in an object, or
    /// dropping/renaming the `preparation-flight-` prefix) -- this id is
    /// journaled by later slices, so its exact wire string is the persisted
    /// meaning, not an implementation detail.
    #[test]
    fn preparation_flight_id_golden_wire_form_round_trips() {
        let id = flight_id('7');
        let golden = format!("\"preparation-flight-{}\"", "7".repeat(64));
        assert_eq!(serde_json::to_string(&id).unwrap(), golden);
        let decoded: PreparationFlightIdV1 = serde_json::from_str(&golden).unwrap();
        assert_eq!(decoded, id);
    }

    /// Discriminates: `parse()` silently accepting a malformed id along any
    /// one of its checked axes (prefix, exact length, all-zero suffix,
    /// charset). Mutation-checked (both reverted before commit): (1)
    /// weakening the length check from `value.len() != ENCODED_LEN` to
    /// `value.len() < ENCODED_LEN` left `long_by_one` green for the wrong
    /// reason (over-length but otherwise well-formed) while `short_by_one`
    /// stayed correctly rejected under the same mutation, confirming the
    /// need for the exact-length (not one-sided) check; (2) deleting the
    /// all-zero-suffix clause independently turned `all_zero_suffix` green
    /// for the wrong reason.
    #[test]
    fn preparation_flight_id_parse_rejects_edges() {
        let expect_err = crate::error::BridgeError::InvalidRequest {
            field: "preparation_flight_id",
        };
        let prefix = PreparationFlightIdV1::PREFIX;
        let cases: &[(&str, String)] = &[
            ("empty", String::new()),
            (
                "wrong_prefix",
                format!("resource-flight-{}", "1".repeat(64)),
            ),
            ("short_by_one", format!("{prefix}{}", "1".repeat(63))),
            ("long_by_one", format!("{prefix}{}", "1".repeat(65))),
            ("all_zero_suffix", format!("{prefix}{}", "0".repeat(64))),
            ("uppercase_hex", format!("{prefix}A{}", "1".repeat(63))),
            ("non_hex_char", format!("{prefix}g{}", "1".repeat(63))),
        ];
        for (name, input) in cases {
            assert_eq!(
                PreparationFlightIdV1::parse(input.clone()),
                Err(expect_err.clone()),
                "case {name} should be rejected"
            );
        }
        // Positive control: the exact-length, non-zero, lowercase-hex boundary parses.
        assert!(PreparationFlightIdV1::parse(format!("{prefix}{}", "1".repeat(64))).is_ok());
    }

    /// Discriminates: `mint()` producing a fixed/predictable id (broken RNG
    /// wiring) or an id that fails its own `parse()` (encoding drift between
    /// `mint()` and `parse()`).
    #[test]
    fn preparation_flight_id_mint_is_random_and_self_parseable() {
        let first = PreparationFlightIdV1::mint().unwrap();
        let second = PreparationFlightIdV1::mint().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.as_str().len(), PreparationFlightIdV1::ENCODED_LEN);
        assert_eq!(PreparationFlightIdV1::parse(first.as_str()).unwrap(), first);
    }

    /// Discriminates: the custom `Deserialize` impl accepting a JSON string
    /// that would be rejected by `parse()` directly (i.e. bypassing
    /// validation on the deserialization path specifically).
    #[test]
    fn preparation_flight_id_deserialize_rejects_what_parse_rejects() {
        assert!(
            serde_json::from_str::<PreparationFlightIdV1>("\"preparation-flight-short\"").is_err()
        );
        assert!(serde_json::from_str::<PreparationFlightIdV1>("123").is_err());
    }

    /// Discriminates: a derive drift that breaks `Ord`/`Hash` (e.g. a manual
    /// impl comparing only part of the id) -- these ids are collected into
    /// `BTreeSet`/`HashSet` in journaled cross-product checks downstream.
    #[test]
    fn preparation_flight_id_ord_and_hash_support_sorted_sets() {
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

    /// Discriminates: any change to the pinned wire form of a plain (already
    /// canonical) transfer reason.
    #[test]
    fn bounded_preparation_transfer_reason_golden_round_trip() {
        let reason = BoundedPreparationTransferReasonV1::new("handoff ok").unwrap();
        assert_eq!(serde_json::to_string(&reason).unwrap(), "\"handoff ok\"");
        let decoded: BoundedPreparationTransferReasonV1 =
            serde_json::from_str("\"handoff ok\"").unwrap();
        assert_eq!(decoded, reason);
    }

    /// Discriminates: `new()` accepting a value that sanitizes down to the
    /// empty string (either literally empty, or entirely stripped control
    /// bytes), which would persist a meaningless transfer reason.
    #[test]
    fn bounded_preparation_transfer_reason_rejects_empty_after_sanitize() {
        let expect_err = crate::error::BridgeError::InvalidRequest {
            field: "preparation_transfer_reason",
        };
        assert_eq!(
            BoundedPreparationTransferReasonV1::new(""),
            Err(expect_err.clone())
        );
        assert_eq!(
            BoundedPreparationTransferReasonV1::new("\u{7}"),
            Err(expect_err)
        );
    }

    /// Discriminates: `new()` not applying the `MAX_PREPARATION_TRANSFER_REASON_BYTES`
    /// bound (an unbounded reason could inflate a journaled record without limit).
    #[test]
    fn bounded_preparation_transfer_reason_truncates_at_max_bytes() {
        let long = "a".repeat(MAX_PREPARATION_TRANSFER_REASON_BYTES + 50);
        let reason = BoundedPreparationTransferReasonV1::new(&long).unwrap();
        assert_eq!(reason.as_str().len(), MAX_PREPARATION_TRANSFER_REASON_BYTES);
        assert_eq!(
            reason.as_str(),
            "a".repeat(MAX_PREPARATION_TRANSFER_REASON_BYTES)
        );
    }

    /// Discriminates: the custom `Deserialize` impl accepting a raw string
    /// that sanitization would rewrite (e.g. embedded control bytes) instead
    /// of enforcing that only the already-canonical form deserializes --
    /// otherwise two distinct wire strings could decode to the same value,
    /// breaking the "wire form is the persisted meaning" invariant.
    /// Mutation-checked: deleting the `parsed.as_str() != value` canonical
    /// check turned this test's first assertion red; reverted before commit.
    #[test]
    fn bounded_preparation_transfer_reason_deserialize_rejects_noncanonical_input() {
        let raw_with_control_byte = "a\u{7}b";
        let encoded = serde_json::to_string(raw_with_control_byte).unwrap();
        assert!(serde_json::from_str::<BoundedPreparationTransferReasonV1>(&encoded).is_err());
        // Sanity: the sanitized form itself (without the control byte) is canonical and parses.
        assert!(serde_json::from_str::<BoundedPreparationTransferReasonV1>("\"ab\"").is_ok());
    }

    /// Discriminates: derive drift on `Ord`/`Hash` for the reason newtype.
    #[test]
    fn bounded_preparation_transfer_reason_ord_and_hash_support_sorted_sets() {
        use std::collections::BTreeSet;
        let a = BoundedPreparationTransferReasonV1::new("a").unwrap();
        let b = BoundedPreparationTransferReasonV1::new("b").unwrap();
        assert!(a < b);
        let sorted: BTreeSet<_> = [b.clone(), a.clone()].into_iter().collect();
        assert_eq!(sorted.into_iter().collect::<Vec<_>>(), vec![a, b]);
    }

    /// Discriminates: drift in the internally-tagged wire shape of the two
    /// unit variants (e.g. `#[serde(tag = "state")]` accidentally dropped, or
    /// `rename_all` removed so the tag value stops being snake_case).
    #[test]
    fn preparation_flight_state_unit_variants_golden_round_trip() {
        assert_eq!(
            serde_json::to_string(&PreparationFlightStateV1::Open {}).unwrap(),
            r#"{"state":"open"}"#
        );
        assert_eq!(
            serde_json::from_str::<PreparationFlightStateV1>(r#"{"state":"open"}"#).unwrap(),
            PreparationFlightStateV1::Open {}
        );
        assert_eq!(
            serde_json::to_string(&PreparationFlightStateV1::BarrierSynced {}).unwrap(),
            r#"{"state":"barrier_synced"}"#
        );
        assert_eq!(
            serde_json::from_str::<PreparationFlightStateV1>(r#"{"state":"barrier_synced"}"#)
                .unwrap(),
            PreparationFlightStateV1::BarrierSynced {}
        );
    }

    /// Discriminates: drift in the `Transferred { reason }` payload shape
    /// (e.g. reason field renamed, or moved under an adjacently-tagged
    /// `"content"` wrapper).
    #[test]
    fn preparation_flight_state_transferred_golden_round_trip() {
        let state = PreparationFlightStateV1::Transferred {
            reason: BoundedPreparationTransferReasonV1::new("handoff ok").unwrap(),
        };
        let golden = r#"{"state":"transferred","reason":"handoff ok"}"#;
        assert_eq!(serde_json::to_string(&state).unwrap(), golden);
        assert_eq!(
            serde_json::from_str::<PreparationFlightStateV1>(golden).unwrap(),
            state
        );
    }

    /// Discriminates: drift in the `Failed { cause }` payload shape, or in
    /// the nested `BoundedCauseV1` (`NodeCauseV1`) field order/names -- this
    /// is the exact bytes a later slice will journal for a failed flight.
    #[test]
    fn preparation_flight_state_failed_golden_round_trip() {
        let state = PreparationFlightStateV1::Failed {
            cause: sample_cause(),
        };
        let golden = "{\"state\":\"failed\",\"cause\":{\"failure_class\":\"persistence\",\
             \"code\":\"bridge.sample_cause\",\"deepest_cause\":\"boom\",\
             \"cause_truncated\":false,\"evidence_overflow\":false,\"dependency_set\":null}}";
        assert_eq!(serde_json::to_string(&state).unwrap(), golden);
        assert_eq!(
            serde_json::from_str::<PreparationFlightStateV1>(golden).unwrap(),
            state
        );
    }

    /// Discriminates: `#[serde(deny_unknown_fields)]` being dropped from
    /// `PreparationFlightStateV1` for its struct-payload variants (a
    /// persisted/journaled contract type must reject unrecognized sibling
    /// keys there, not silently ignore them). Uses `Transferred`, a
    /// struct-payload variant -- see the test immediately below, which
    /// covers the same guarantee for the (as of this slice) empty-struct
    /// variant `Open`.
    #[test]
    fn preparation_flight_state_rejects_unknown_field_on_struct_variant() {
        assert!(serde_json::from_str::<PreparationFlightStateV1>(
            r#"{"state":"transferred","reason":"handoff ok","extra":1}"#
        )
        .is_err());
    }

    /// R2f1b F-2 tightening: `#[serde(tag = "state", deny_unknown_fields)]`
    /// on an internally-tagged enum only enforces `deny_unknown_fields` for
    /// struct-payload variants -- a bare unit variant like `Open`/
    /// `BarrierSynced` used to deserialize without ever checking whether the
    /// buffered JSON object had leftover keys, tightened per the A2-review
    /// pre-slice-3 obligation recorded in the custody plan ledger
    /// (`docs/superpowers/plans/2026-08-06-r2f1b-pre-slice-2-custody-plan.md`).
    /// `Open` and `BarrierSynced` are now declared as empty-struct payloads
    /// (`Open {}`) instead of unit variants: the wire form is byte-identical
    /// (`{"state":"open"}`, see the golden round-trip test above), but the
    /// derive now buffers the variant's fields like any other struct
    /// payload, so a stray sibling key is rejected here too. This closes
    /// the gap the FINDING test used to pin.
    #[test]
    fn preparation_flight_state_rejects_unknown_field_on_unit_variant() {
        assert!(
            serde_json::from_str::<PreparationFlightStateV1>(r#"{"state":"open","extra":1}"#)
                .is_err()
        );
    }

    /// Discriminates: an unrecognized `"state"` tag value being silently
    /// accepted (e.g. into a default variant) instead of rejected.
    #[test]
    fn preparation_flight_state_rejects_unknown_variant_tag() {
        assert!(serde_json::from_str::<PreparationFlightStateV1>(r#"{"state":"bogus"}"#).is_err());
    }

    /// R2f1b slice-2a tightening (was the A2 FINDING test, which pinned the
    /// gap by asserting `is_ok()`): `PreparationFlightStateV1`'s
    /// `deny_unknown_fields` guards only its *own* top-level keys ("state",
    /// "cause") -- it does not propagate into the nested `cause` object's
    /// field set, and `BoundedCauseV1` (`NodeCauseV1` in execution_policy.rs)
    /// carried no `deny_unknown_fields` of its own. `NodeCauseV1` now does
    /// (see `execution_policy::r2f1b_node_cause_strictness_tests`), so an
    /// unrecognized key inside `cause` is rejected on this plain-serde path
    /// too -- which matters here precisely because, unlike the durable node
    /// records, this contract has no canonical re-encode check to catch it.
    /// Discriminates: that attribute being dropped again; the positive control
    /// below pins that the well-formed nested cause still decodes.
    #[test]
    fn preparation_flight_state_failed_cause_rejects_unknown_nested_field() {
        let json = "{\"state\":\"failed\",\"cause\":{\"failure_class\":\"persistence\",\
             \"code\":\"bridge.sample_cause\",\"deepest_cause\":\"boom\",\
             \"cause_truncated\":false,\"evidence_overflow\":false,\"dependency_set\":null,\
             \"unexpected_extra_field\":\"leaked\"}}";
        assert!(serde_json::from_str::<PreparationFlightStateV1>(json).is_err());

        let without_extra = json.replace(",\"unexpected_extra_field\":\"leaked\"", "");
        assert!(serde_json::from_str::<PreparationFlightStateV1>(&without_extra).is_ok());
    }

    struct FakeClock(AtomicU64);
    impl MonotonicClock for FakeClock {
        fn elapsed_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// Discriminates: `PreparationClockV1::elapsed_ms` not delegating to the
    /// wrapped clock (e.g. a stray hardcoded value), and its custom `Debug`
    /// impl leaking the wrapped clock's internal state instead of the fixed
    /// `"PreparationClockV1 { .. }"` redaction placeholder.
    #[test]
    fn preparation_clock_v1_delegates_elapsed_ms_and_debug_does_not_leak() {
        let clock = PreparationClockV1::new(Arc::new(FakeClock(AtomicU64::new(42))));
        assert_eq!(clock.elapsed_ms(), 42);
        assert_eq!(format!("{clock:?}"), "PreparationClockV1 { .. }");
    }
}
