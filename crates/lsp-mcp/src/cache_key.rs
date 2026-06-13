//! Per-repo CARGO_TARGET_DIR derivation. The key is a *reuse boundary*, not a re-index trigger:
//! clones of the same repo (same origin) share one warm build cache -> ~0.7s usable index vs ~9s cold.
use std::path::{Path, PathBuf};

/// Per-repo cache dir under `base`, keyed by `origin_url` (git remote.origin.url) when present and
/// non-blank, else by `repo_root`'s path. Dir name is FS-safe hex.
pub fn cache_dir(base: &Path, repo_root: &Path, origin_url: Option<&str>) -> PathBuf {
    let key = match origin_url {
        Some(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => repo_root.to_string_lossy().into_owned(),
    };
    base.join(format!("ra-{}", fnv1a_hex(&key)))
}

fn fnv1a_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn same_origin_same_dir_regardless_of_clone_path() {
        let base = Path::new("/cache");
        let a = cache_dir(
            base,
            Path::new("/clones/A"),
            Some("git@github.com:me/repo.git"),
        );
        let b = cache_dir(
            base,
            Path::new("/clones/B"),
            Some("git@github.com:me/repo.git"),
        );
        assert_eq!(a, b, "clones of the same origin must share one target dir");
        // NB: `Path::starts_with` is component-wise (false for a hex-suffixed leaf); compare as a string.
        assert!(a.to_str().unwrap().starts_with("/cache/ra-"));
    }

    #[test]
    fn different_origin_different_dir() {
        let base = Path::new("/cache");
        let a = cache_dir(base, Path::new("/x"), Some("git@github.com:me/one.git"));
        let b = cache_dir(base, Path::new("/x"), Some("git@github.com:me/two.git"));
        assert_ne!(a, b);
    }

    #[test]
    fn blank_or_missing_origin_falls_back_to_path() {
        let base = Path::new("/cache");
        let by_path = cache_dir(base, Path::new("/clones/A"), None);
        let by_blank = cache_dir(base, Path::new("/clones/A"), Some("   "));
        assert_eq!(by_path, by_blank, "blank origin must behave like no origin");
        let other = cache_dir(base, Path::new("/clones/B"), None);
        assert_ne!(by_path, other, "path-keyed dirs differ by path");
    }
}
