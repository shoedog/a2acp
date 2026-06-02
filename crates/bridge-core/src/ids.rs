// ids.rs — parse-don't-validate newtypes for domain identifiers (spec §5.1/§5.4).

use crate::error::BridgeError;

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
id_newtype!(SessionId);
id_newtype!(CallerId);
id_newtype!(AgentId);

macro_rules! id_newtype_strict {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

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
