use bridge_core::error::BridgeError;
use bridge_core::SessionCwd;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct WorktreeConfig {
    pub root: String,
    pub owner: String,
    pub run: String,
}

pub struct ResolvedWorktree {
    pub canonical_source: String,
    pub worktree_path: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct WorktreeSidecar {
    pub canonical_source: String,
    pub common_dir: String,
    pub worktree_path: String,
    pub owner: String,
    pub run_id: String,
    pub host: String,
    pub lease: String,
}

pub fn resolve_worktree(
    cfg: &WorktreeConfig,
    allowed_root: &Option<SessionCwd>,
    repo: &str,
    session_id: &str,
) -> Result<ResolvedWorktree, BridgeError> {
    let canonical_source = canonicalize_existing(repo)?;
    let allowed_root = allowed_root.as_ref().ok_or(BridgeError::InvalidRequest {
        field: "worktrees requires allowed_cwd_root",
    })?;
    let canonical_allowed_root = canonicalize_lenient(allowed_root.as_str())?;
    if !canonical_source.is_under(&canonical_allowed_root) {
        return Err(BridgeError::InvalidRequest {
            field: "worktree source outside allowed_cwd_root",
        });
    }

    let canonical_cfg_root = canonicalize_lenient(&cfg.root)?;
    let worktree_name = format!(
        "{}-{}-{}",
        cfg.owner,
        cfg.run,
        session_hash_suffix(session_id)
    );
    let worktree_path = Path::new(canonical_cfg_root.as_str()).join(worktree_name);
    let canonical_worktree = canonicalize_lenient(&worktree_path.to_string_lossy())?;

    Ok(ResolvedWorktree {
        canonical_source: canonical_source.as_str().to_string(),
        worktree_path: canonical_worktree.as_str().to_string(),
    })
}

pub fn sidecar_path(worktree_path: &str) -> String {
    format!("{worktree_path}.meta.json")
}

pub fn write_sidecar(s: &WorktreeSidecar) -> Result<(), BridgeError> {
    let path = sidecar_path(&s.worktree_path);
    let tmp = format!("{path}.tmp");
    let json = serde_json::to_vec(s).map_err(|_| BridgeError::StoreFailure)?;
    std::fs::write(&tmp, json).map_err(|_| BridgeError::StoreFailure)?;
    std::fs::rename(&tmp, &path).map_err(|_| {
        let _ = std::fs::remove_file(&tmp);
        BridgeError::StoreFailure
    })
}

pub fn read_sidecar(path: &str) -> Option<WorktreeSidecar> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn session_hash_suffix(session_id: &str) -> String {
    let mut h = DefaultHasher::new();
    session_id.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn canonicalize_existing(path: &str) -> Result<SessionCwd, BridgeError> {
    let canonical = std::fs::canonicalize(path).map_err(|_| BridgeError::ConfigInvalid {
        reason: format!("worktree source has no canonical root: {path}"),
    })?;
    SessionCwd::parse(&canonical.to_string_lossy())
}

/// Canonicalize `path`, resolving symlinks. If it doesn't exist yet, canonicalize the nearest
/// existing ancestor and re-append the missing tail.
pub(crate) fn canonicalize_lenient(path: &str) -> Result<SessionCwd, BridgeError> {
    let p = Path::new(path);
    let mut existing = p;
    let mut tail: Vec<std::ffi::OsString> = vec![];
    let canon = loop {
        match std::fs::canonicalize(existing) {
            Ok(c) => break c,
            Err(_) => {
                let file = existing
                    .file_name()
                    .ok_or_else(|| BridgeError::ConfigInvalid {
                        reason: format!("worktree path has no canonical root: {path}"),
                    })?
                    .to_os_string();
                tail.push(file);
                existing = existing
                    .parent()
                    .ok_or_else(|| BridgeError::ConfigInvalid {
                        reason: format!("worktree path has no canonical root: {path}"),
                    })?;
            }
        }
    };
    let mut out: PathBuf = canon;
    for seg in tail.iter().rev() {
        out.push(seg);
    }
    SessionCwd::parse(&out.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "a2a-bridge-worktree-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn cfg(root: &str) -> WorktreeConfig {
        WorktreeConfig {
            root: root.into(),
            owner: "ownr".into(),
            run: "run7".into(),
        }
    }

    #[test]
    fn resolve_worktree_gates_and_scopes() {
        let allowed_root = unique_temp_dir("allowed");
        let source = allowed_root.join("source");
        let worktree_root = unique_temp_dir("root");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&worktree_root).unwrap();

        let allowed_cwd = Some(SessionCwd::parse(&allowed_root.to_string_lossy()).unwrap());
        let cfg = cfg(&worktree_root.to_string_lossy());
        let resolved =
            resolve_worktree(&cfg, &allowed_cwd, &source.to_string_lossy(), "ctx-c1-g0").unwrap();

        let canonical_source = fs::canonicalize(&source)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let canonical_root = fs::canonicalize(&worktree_root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(resolved.canonical_source, canonical_source);
        assert!(
            resolved
                .worktree_path
                .starts_with(&format!("{canonical_root}/ownr-run7-")),
            "owner+run+hash scoped under canonical root: {}",
            resolved.worktree_path
        );

        let outside = unique_temp_dir("outside");
        fs::create_dir_all(&outside).unwrap();
        assert!(
            resolve_worktree(&cfg, &allowed_cwd, &outside.to_string_lossy(), "ctx-c2-g0").is_err(),
            "source outside allowed_root rejected"
        );
        assert!(
            resolve_worktree(&cfg, &None, &source.to_string_lossy(), "ctx-c1-g0").is_err(),
            "allowed_root is required"
        );

        let again =
            resolve_worktree(&cfg, &allowed_cwd, &source.to_string_lossy(), "ctx-c1-g0").unwrap();
        assert_eq!(resolved.worktree_path, again.worktree_path);

        fs::remove_dir_all(&allowed_root).unwrap();
        fs::remove_dir_all(&worktree_root).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn sidecar_round_trips() {
        let worktree_root = unique_temp_dir("sidecar");
        fs::create_dir_all(&worktree_root).unwrap();
        let wt = worktree_root.join("ownr-run7-abc");
        let sidecar = WorktreeSidecar {
            canonical_source: "/repo".into(),
            common_dir: "/repo/.git".into(),
            worktree_path: wt.to_string_lossy().into_owned(),
            owner: "ownr".into(),
            run_id: "run-id".into(),
            host: "host-a".into(),
            lease: "/tmp/a2a.lock".into(),
        };

        write_sidecar(&sidecar).unwrap();
        assert_eq!(
            read_sidecar(&sidecar_path(&sidecar.worktree_path)),
            Some(sidecar)
        );

        assert_eq!(
            read_sidecar(&worktree_root.join("missing.meta.json").to_string_lossy()),
            None
        );
        let garbage = worktree_root.join("garbage.meta.json");
        fs::write(&garbage, "not json").unwrap();
        assert_eq!(read_sidecar(&garbage.to_string_lossy()), None);

        fs::remove_dir_all(&worktree_root).unwrap();
    }
}
