use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct ConfigError(pub String);

pub struct Mount {
    pub host: PathBuf,
}

/// Optional per-agent overrides layered on top of the base sandbox config.
pub struct Overrides {
    pub extra_mounts: Vec<Mount>,
}

pub struct SandboxConfig {
    pub allowed_root: PathBuf,
    pub mounts: Vec<Mount>,
    pub overrides: Option<Overrides>,
}

impl SandboxConfig {
    fn under_root(&self, p: &Path) -> bool {
        p.starts_with(&self.allowed_root)
    }

    /// Reject any config whose mounts escape `allowed_root`.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for m in &self.mounts {
            if !self.under_root(&m.host) {
                return Err(ConfigError(format!(
                    "mount {:?} escapes allowed_root {:?}",
                    m.host, self.allowed_root
                )));
            }
        }
        Ok(())
    }

    /// The full set of host paths this config will bind-mount at runtime.
    pub fn effective_mounts(&self) -> Vec<&Path> {
        let mut out: Vec<&Path> = self.mounts.iter().map(|m| m.host.as_path()).collect();
        if let Some(ov) = &self.overrides {
            out.extend(ov.extra_mounts.iter().map(|m| m.host.as_path()));
        }
        out
    }
}
