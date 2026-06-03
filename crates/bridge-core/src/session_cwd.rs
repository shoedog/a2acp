// session_cwd.rs — parse-don't-validate newtype for an ACP session working directory (§11A).

use crate::error::BridgeError;
use std::path::{Component, Path, PathBuf};

/// A validated absolute, lexically-normalized session working directory (ACP §11A).
/// Construct ONLY via [`SessionCwd::parse`]; holding one guarantees validity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCwd(String);

impl SessionCwd {
    pub fn parse(s: &str) -> Result<SessionCwd, BridgeError> {
        if s.is_empty() || s.contains('\0') {
            return Err(BridgeError::InvalidRequest {
                field: "a2a-bridge.cwd",
            });
        }
        let p = Path::new(s);
        if !p.is_absolute() {
            return Err(BridgeError::InvalidRequest {
                field: "a2a-bridge.cwd",
            });
        }
        // Lexical normalization: fold . and .. without touching the filesystem.
        let mut out = PathBuf::new();
        for comp in p.components() {
            match comp {
                Component::RootDir | Component::Prefix(_) => out.push(comp.as_os_str()),
                Component::CurDir => {}
                Component::ParentDir => {
                    // pop a Normal component; refuse to escape above root
                    if !out.pop() || out.as_os_str().is_empty() {
                        return Err(BridgeError::InvalidRequest {
                            field: "a2a-bridge.cwd",
                        });
                    }
                }
                Component::Normal(seg) => out.push(seg),
            }
        }
        Ok(SessionCwd(out.to_string_lossy().into_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Component-wise prefix check on two already-normalized absolute paths
    /// (NOT a string prefix — `/work-evil` is NOT under `/work`). Lexical only:
    /// this is a path-shape guard, not a sandbox (symlinks are not resolved).
    pub fn is_under(&self, root: &SessionCwd) -> bool {
        let self_comps: Vec<_> = Path::new(&self.0).components().collect();
        let root_comps: Vec<_> = Path::new(&root.0).components().collect();
        self_comps.starts_with(&root_comps)
    }
}

impl std::fmt::Display for SessionCwd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cwd_parse_rules() {
        assert!(SessionCwd::parse("/abs/repo").is_ok());
        assert_eq!(SessionCwd::parse("/a/b/../c").unwrap().as_str(), "/a/c"); // lexical ..-collapse
        assert_eq!(SessionCwd::parse("/a/./b").unwrap().as_str(), "/a/b"); // . dropped
        assert!(SessionCwd::parse("rel/path").is_err()); // not absolute
        assert!(SessionCwd::parse("").is_err()); // empty
        assert!(SessionCwd::parse("/a\0b").is_err()); // NUL
        assert!(SessionCwd::parse("/a/../..").is_err()); // escapes above root
    }

    #[test]
    fn session_cwd_is_under() {
        let root = SessionCwd::parse("/work").unwrap();
        assert!(SessionCwd::parse("/work/repo").unwrap().is_under(&root));
        assert!(SessionCwd::parse("/work").unwrap().is_under(&root)); // equal is under
        assert!(!SessionCwd::parse("/work-evil").unwrap().is_under(&root)); // component-wise, not str prefix
        assert!(!SessionCwd::parse("/other").unwrap().is_under(&root));
    }
}
