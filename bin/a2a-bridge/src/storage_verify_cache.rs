//! Volume maintenance is separate from filesystem-target reaping. Every observation and mutation
//! uses verify's lock and runner; unknown consumers or failed measurement retain the volume.
use bridge_controller::verify::{self, Runner, VerifyConfig};

#[derive(Debug, serde::Serialize)]
pub struct Usage {
    pub dry_run: bool,
    pub cache_modes: Vec<String>,
    pub runtime: String,
    pub volume: String,
    pub measured_bytes: Option<u64>,
    pub logical_bytes: Option<u64>,
    pub disk_bytes: Option<u64>,
    pub limit_bytes: u64,
    pub excess_bytes: Option<u64>,
    pub proposed_bytes: Option<u64>,
    pub retained_bytes: Option<u64>,
    pub reclaimed_bytes: Option<u64>,
    pub deferred_bytes: Option<u64>,
    pub error: Option<String>,
}

impl Usage {
    pub fn note(&self) -> String {
        let number = |n: Option<u64>| n.map(|n| n.to_string()).unwrap_or_else(|| "unknown".into());
        format!("{} volume {}; {}; accounted bytes={} (max logical/allocated per entry); limit={}; excess={}; proposed={}; retained={}; reclaimed={}; deferred={}; {}{}",
            self.runtime, self.volume, if self.dry_run { "preview" } else { "apply" },
            number(self.measured_bytes), self.limit_bytes, number(self.excess_bytes),
            number(self.proposed_bytes), number(self.retained_bytes), number(self.reclaimed_bytes),
            number(self.deferred_bytes),
            self.cache_modes.join("; "), self.error.as_ref().map(|e| format!("; deferred: {e}")).unwrap_or_default())
    }
}

pub fn eligible(base: &str, volume: &str) -> bool {
    if !volume
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        return false;
    }
    volume
        .strip_prefix(base)
        .and_then(|s| s.strip_prefix('-'))
        .is_some_and(|suffix| suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub fn inspect(cfg: &VerifyConfig, volume: &str, runner: &Runner, apply: bool) -> Usage {
    let runtime = cfg.runtime.as_deref().unwrap_or("docker");
    let mut row = Usage {
        dry_run: !apply,
        cache_modes: Vec::new(),
        runtime: runtime.into(),
        volume: volume.into(),
        measured_bytes: None,
        logical_bytes: None,
        disk_bytes: None,
        limit_bytes: verify::VERIFY_CACHE_LIMIT_BYTES,
        excess_bytes: None,
        proposed_bytes: None,
        retained_bytes: None,
        reclaimed_bytes: None,
        deferred_bytes: None,
        error: None,
    };
    let operation = || -> std::io::Result<verify::CacheMeasurement> {
        if !eligible(&cfg.cache, volume) {
            return Err(std::io::Error::other(
                "volume does not match configured verify cache",
            ));
        }
        // Manual maintenance is non-blocking: a live verify is a deferred row, never a deletion.
        let _lock = bridge_core::liveness::acquire_persistent_lock_in(
            &verify::cache_volume_lock_dir(std::path::Path::new("/"))?,
            &verify::cache_volume_lock_id(volume),
        )?;
        verify::cache_consumers_idle(runtime, volume, runner)?;
        verify::maintain_cache(cfg, volume, runner, !apply)
    };
    match operation() {
        Ok(m) => {
            row.measured_bytes = Some(m.before);
            row.logical_bytes = m.logical_bytes;
            row.disk_bytes = m.disk_bytes;
            row.excess_bytes = Some(m.before.saturating_sub(row.limit_bytes));
            row.proposed_bytes = Some(m.proposed);
            row.retained_bytes = Some(m.after);
            row.reclaimed_bytes = Some(m.before.saturating_sub(m.after));
            row.deferred_bytes = Some(m.after.saturating_sub(row.limit_bytes));
            row.error = m.error;
        }
        Err(e) => row.error = Some(e.to_string()),
    }
    row
}

pub fn collect(raw: &str, runner: &Runner, apply: bool) -> Result<Vec<Usage>, crate::BoxError> {
    let cfg = crate::config::RegistryConfig::parse(raw)?;
    let profiles = cfg.language_profiles()?;
    let profiles = if profiles.is_empty() {
        vec![bridge_core::profile::rust_profile()]
    } else {
        profiles
    };
    let cache_modes: Vec<String> = profiles
        .iter()
        .filter(|p| p.id == "rust")
        .map(|p| {
            verify::rust_cache_mode(&p.verify_commands)
                .unwrap_or_else(|e| format!("cache: refused ({e})"))
        })
        .collect();
    let Some(v) = cfg.verify else {
        return Ok(Vec::new());
    };
    let cfg = v.to_config()?;
    let runtime = cfg.runtime.as_deref().unwrap_or("docker");
    let (status, out) = runner(
        runtime,
        &[
            "volume".into(),
            "ls".into(),
            "--format".into(),
            "{{.Name}}".into(),
        ],
    )?;
    if status != 0 {
        return Err(format!("verify-cache: {runtime} volume list failed: {out}").into());
    }
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|name| eligible(&cfg.cache, name))
        .map(|name| {
            let mut row = inspect(&cfg, name, runner, apply);
            row.cache_modes = cache_modes.clone();
            row
        })
        .collect())
}

pub fn command(args: &[String]) -> Result<(), crate::BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("storage reap --verify-cache --config PATH (--dry-run | --apply) [--json]\n\
            Trim configured repository verify volumes to 20 GiB. Preview mounts payload read-only.\n\
            Live/unknown consumers, lock errors and measurement errors defer maintenance.\n\
            Rust commands use shared or isolated targets inside the volume; visible overrides refuse.\n\
            Manual maintenance uses [verify].image; it must provide Python 3. Language image\n\
            overrides apply when a language is selected for automated verification.");
        return Ok(());
    }
    let mut path = None;
    let mut preview = false;
    let mut apply = false;
    let mut json = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--verify-cache" => {}
            "--config" => {
                path = Some(std::path::PathBuf::from(
                    it.next().ok_or("--config needs a path")?,
                ))
            }
            "--dry-run" => preview = true,
            "--apply" => apply = true,
            "--json" => json = true,
            _ => return Err(format!("verify-cache: unknown or mixed reap flag {arg}").into()),
        }
    }
    if preview == apply {
        return Err("verify-cache: choose exactly one of --dry-run or --apply".into());
    }
    let path = crate::require_config_path(path)?;
    let raw = std::fs::read_to_string(path)?;
    let rows = collect(&raw, &crate::docker_runner, apply)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            println!("{}", row.note());
        }
    }
    if rows.iter().any(|r| r.error.is_some()) {
        return Err("verify-cache: maintenance deferred; inspect reported errors".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_configured_repository_volumes_are_eligible() {
        assert!(eligible("custom", "custom-0123456789abcdef"));
        for name in [
            "custom",
            "custom-target",
            "other-0123456789abcdef",
            "custom-../tmp",
            "custom-0123456789abcdef-extra",
        ] {
            assert!(!eligible("custom", name), "{name}");
        }
    }

    #[test]
    fn rejects_mixed_classes_and_ambiguous_effects_before_config_access() {
        for flags in [
            vec!["--verify-cache"],
            vec!["--verify-cache", "--dry-run", "--apply"],
            vec!["--verify-cache", "--dry-run", "--clones"],
            vec!["--verify-cache", "--apply", "--limit", "0"],
        ] {
            assert!(command(&flags.into_iter().map(String::from).collect::<Vec<_>>()).is_err());
        }
    }
}
