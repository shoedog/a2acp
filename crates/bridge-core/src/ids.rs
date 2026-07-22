// ids.rs — parse-don't-validate newtypes for domain identifiers (spec §5.1/§5.4).

use crate::error::BridgeError;
use ring::rand::{SecureRandom, SystemRandom};

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        pub struct $name(String);
        impl $name {
            pub fn parse(s: impl Into<String>) -> Result<Self, BridgeError> {
                let s = s.into();
                if s.is_empty() {
                    return Err(BridgeError::InvalidRequest {
                        field: stringify!($name),
                    });
                }
                Ok(Self(s))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

id_newtype!(TaskId);
id_newtype!(BatchId);
id_newtype!(SessionId);
id_newtype!(CallerId);
id_newtype!(AgentId);

// Slice 0 (orchestration) ids.
id_newtype!(SessionHandleId);
id_newtype!(SessionHandleRef);
id_newtype!(OperationId);
id_newtype!(ContextId);
id_newtype!(TurnId);
id_newtype!(SourceId);

/// A warm session's context generation. Hand-written (the `id_newtype!` macros are
/// String-only); generations are compared/incremented so we add `Copy`/`Ord`.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct SessionGeneration(pub u64);
impl SessionGeneration {
    pub fn new(n: u64) -> Self {
        Self(n)
    }
    pub fn get(&self) -> u64 {
        self.0
    }
}

macro_rules! id_newtype_strict {
    ($name:ident) => {
        #[derive(
            Clone,
            Debug,
            PartialEq,
            Eq,
            Hash,
            PartialOrd,
            Ord,
            serde::Serialize,
            serde::Deserialize,
        )]
        pub struct $name(String);
        impl $name {
            /// Validated id: non-empty and `[a-z0-9_-]+` only. Stricter than the plain
            /// id_newtype because these ids are interpolated into `{{<id>}}` template tokens.
            pub fn parse(s: impl Into<String>) -> Result<Self, BridgeError> {
                let s = s.into();
                if s.is_empty()
                    || !s.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                    })
                {
                    return Err(BridgeError::InvalidRequest {
                        field: stringify!($name),
                    });
                }
                Ok(Self(s))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}
id_newtype_strict!(WorkflowId);
id_newtype_strict!(NodeId);

macro_rules! high_entropy_id {
    ($name:ident, $prefix:literal, $field:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);
        impl $name {
            pub const PREFIX: &'static str = $prefix;
            pub const ENCODED_LEN: usize = $prefix.len() + 32;
            pub fn mint() -> Result<Self, BridgeError> {
                let mut bytes = [0_u8; 16];
                SystemRandom::new()
                    .fill(&mut bytes)
                    .map_err(|_| BridgeError::IdentityUnavailable)?;
                let mut value = String::with_capacity(Self::ENCODED_LEN);
                value.push_str($prefix);
                for byte in bytes {
                    use std::fmt::Write as _;
                    let _ = write!(&mut value, "{byte:02x}");
                }
                Ok(Self(value))
            }
            pub fn parse(value: impl Into<String>) -> Result<Self, BridgeError> {
                let value = value.into();
                let suffix = value
                    .strip_prefix($prefix)
                    .ok_or(BridgeError::InvalidRequest { field: $field })?;
                if value.len() != Self::ENCODED_LEN
                    || suffix.bytes().all(|b| b == b'0')
                    || !suffix
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(BridgeError::InvalidRequest { field: $field });
                }
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = <String as serde::Deserialize>::deserialize(deserializer)?;
                Self::parse(value).map_err(serde::de::Error::custom)
            }
        }
    };
}
high_entropy_id!(ExecutionId, "exec-", "execution_id");
high_entropy_id!(AttemptId, "attempt-", "attempt_id");

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttemptIdentity {
    pub execution_id: ExecutionId,
    pub attempt_id: AttemptId,
    pub ordinal: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_attempt_id: Option<AttemptId>,
}
impl AttemptIdentity {
    pub fn initial() -> Result<Self, BridgeError> {
        Ok(Self {
            execution_id: ExecutionId::mint()?,
            attempt_id: AttemptId::mint()?,
            ordinal: 0,
            parent_attempt_id: None,
        })
    }
    pub fn resume(&self) -> Result<Self, BridgeError> {
        Ok(Self {
            execution_id: self.execution_id.clone(),
            attempt_id: AttemptId::mint()?,
            ordinal: self
                .ordinal
                .checked_add(1)
                .ok_or(BridgeError::InvalidRequest {
                    field: "attempt ordinal overflow",
                })?,
            parent_attempt_id: Some(self.attempt_id.clone()),
        })
    }
    pub fn run_id(&self) -> &str {
        self.attempt_id.as_str()
    }
}

/// Prompt registry id (E8a). Deliberately MORE permissive than `id_newtype_strict!` (admits uppercase,
/// `/`, `.`) so E8b namespaced partials (`_preamble/review-readonly`) need no grammar change. Derives
/// `Ord` so it can key a `BTreeMap` (the resolved registry / `prompt list` ordering).
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct PromptId(String);
impl PromptId {
    pub fn parse(s: impl Into<String>) -> Result<Self, BridgeError> {
        let s = s.into();
        let trimmed = s.trim();
        let ok = !trimmed.is_empty()
            && trimmed.len() == s.len() // no leading/trailing whitespace
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'));
        if !ok {
            return Err(BridgeError::InvalidRequest { field: "PromptId" });
        }
        Ok(Self(s))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod slice0_id_tests {
    use super::*;
    #[test]
    fn new_orch_ids_parse_and_roundtrip() {
        assert_eq!(SessionHandleId::parse("h-1").unwrap().as_str(), "h-1");
        assert_eq!(OperationId::parse("op-1").unwrap().as_str(), "op-1");
        assert_eq!(ContextId::parse("ctx-1").unwrap().as_str(), "ctx-1");
        assert!(ContextId::parse("").is_err());
    }

    #[test]
    fn session_and_source_newtypes_roundtrip() {
        let s = SessionHandleRef::parse("h-1").unwrap();
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j, serde_json::json!("h-1"));
        assert_eq!(serde_json::from_value::<SessionHandleRef>(j).unwrap(), s);
        assert!(SourceId::parse("src-1").is_ok());
    }

    #[test]
    fn session_generation_orders_and_increments() {
        let g0 = SessionGeneration::new(0);
        let g1 = SessionGeneration::new(g0.get() + 1);
        assert!(g1 > g0);
        assert_eq!(g1.get(), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_id_accepts_namespaced_and_mixed_case_rejects_blank_and_ws() {
        for ok in [
            "review-correctness",
            "_preamble/review-readonly",
            "design.synth",
            "Smoke_Read",
        ] {
            assert!(PromptId::parse(ok).is_ok(), "{ok} should parse");
        }
        for bad in ["", "  ", "a b", "tab\there", "ctrl\u{0}x"] {
            assert!(PromptId::parse(bad).is_err(), "{bad:?} should reject");
        }
        // Ord is derivable -> usable as a BTreeMap key (compile + order check).
        let mut m = std::collections::BTreeMap::new();
        m.insert(PromptId::parse("b").unwrap(), 1);
        m.insert(PromptId::parse("a").unwrap(), 2);
        assert_eq!(m.keys().next().unwrap().as_str(), "a");

        let namespaced = PromptId::parse("_preamble/review-readonly").unwrap();
        let mut by_id = std::collections::BTreeMap::new();
        by_id.insert(namespaced.clone(), "ok");
        assert_eq!(by_id.get(&namespaced), Some(&"ok"));
    }

    #[test]
    fn parses_nonempty_rejects_empty() {
        assert!(SessionId::parse("abc").is_ok());
        assert_eq!(
            SessionId::parse("").unwrap_err(),
            crate::error::BridgeError::InvalidRequest { field: "SessionId" }
        );
    }

    #[test]
    fn as_str_roundtrips() {
        assert_eq!(TaskId::parse("t1").unwrap().as_str(), "t1");
    }

    #[test]
    fn all_four_id_types_parse_and_reject_empty() {
        for ok in [
            TaskId::parse("a").is_ok(),
            SessionId::parse("a").is_ok(),
            CallerId::parse("a").is_ok(),
            AgentId::parse("a").is_ok(),
        ] {
            assert!(ok);
        }
        assert!(TaskId::parse("").is_err());
        assert!(CallerId::parse("").is_err());
        assert!(AgentId::parse("").is_err());
    }

    #[test]
    fn ids_are_hashable_and_eq() {
        use std::collections::HashSet;
        let mut s = HashSet::new();
        s.insert(TaskId::parse("x").unwrap());
        assert!(s.contains(&TaskId::parse("x").unwrap()));
    }

    #[test]
    fn strict_ids_reject_non_charset() {
        assert!(WorkflowId::parse("code-review").is_ok());
        assert!(NodeId::parse("synth_1").is_ok());
        assert!(WorkflowId::parse("").is_err());
        assert!(NodeId::parse("has space").is_err());
        assert!(NodeId::parse("br{{ace").is_err());
        assert!(WorkflowId::parse("UPPER").is_err()); // lowercase only
    }
}

#[cfg(test)]
mod r2f0a_identity_tests {
    use super::*;

    #[test]
    fn identities_are_distinct_validated_and_json_cannot_bypass_validation() {
        let first = AttemptIdentity::initial().unwrap();
        let second = AttemptIdentity::initial().unwrap();
        assert_ne!(first.execution_id, second.execution_id);
        assert_ne!(first.attempt_id, second.attempt_id);
        assert_eq!(first.execution_id.as_str().len(), ExecutionId::ENCODED_LEN);
        for invalid in [
            "",
            "exec-1",
            "exec-00000000000000000000000000000000",
            "exec-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ] {
            assert!(ExecutionId::parse(invalid).is_err());
        }
        assert!(serde_json::from_str::<AttemptId>("\"attempt-short\"").is_err());
    }

    #[test]
    fn resume_preserves_execution_and_links_a_fresh_attempt() {
        let first = AttemptIdentity::initial().unwrap();
        let next = first.resume().unwrap();
        assert_eq!(next.execution_id, first.execution_id);
        assert_ne!(next.attempt_id, first.attempt_id);
        assert_eq!(next.ordinal, 1);
        assert_eq!(next.parent_attempt_id.as_ref(), Some(&first.attempt_id));
        assert_eq!(next.run_id(), next.attempt_id.as_str());
    }
}
