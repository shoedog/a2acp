//! The single cache/env seam (C2 §2.2). One place maps a (language profile, container context) to the
//! cache env + volume mounts to apply — consumed by warm-fetch, verify, and the in-container-lsp mount,
//! replacing three independently-hardcoded cargo sites. Step 1 hardcodes a `rust` profile (byte-for-byte);
//! Step 2 makes `LanguageProfile` config-parsed + adds `go`.

/// A container context that needs language-specific cache env + mounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheCtx {
    /// The warm-deps fetch container (populates the dep cache; must reach the network).
    Fetch,
    /// The in-container language server (reads the dep cache; offline).
    Lsp,
    /// The verify container (build/test against a persistent cache).
    Verify,
}

/// The env + volume mounts a profile contributes for one context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheBinding {
    /// `(key, value)` pairs to export in the container.
    pub env: Vec<(String, String)>,
    /// Docker `-v` specs, e.g. `"vol:/path"` or `"vol:/path:ro"`.
    pub mounts: Vec<String>,
}

/// A per-language profile (an ATOM — selected as a set; never per-combo; C2 §1). Step 1 carries only the
/// fields the seam + warm-fetch consume; Step 2 extends it (verify commands, image override, config parse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageProfile {
    pub id: String,
    /// The warm-deps command (the dep fetch).
    pub fetch_cmd: String,
    /// The cache-volume BASE name the fetch fills (the per-repo suffix is appended by the caller).
    pub warm_cache_base: String,
    /// Where the dep cache mounts in the Fetch (rw) + Lsp (ro) containers.
    dep_cache_path: String,
    /// Where the verify cache mounts in the Verify container.
    verify_cache_path: String,
    /// Env exported in the Fetch container (network-capable — NO offline flag).
    fetch_env: Vec<(String, String)>,
    /// Env exported in the Lsp container. Empty in Step 1 (the lsp env is still config-side).
    lsp_env: Vec<(String, String)>,
    /// Env exported in the Verify container.
    verify_env: Vec<(String, String)>,
}

impl LanguageProfile {
    /// PURE + TOTAL. The env + mounts for `ctx`, given the resolved per-repo `warm_vol` (the dep cache)
    /// and `verify_vol` (the verify cache). Fetch mounts the dep cache rw; Lsp mounts it ro; Verify mounts
    /// the verify cache.
    pub fn cache_binding(&self, ctx: CacheCtx, warm_vol: &str, verify_vol: &str) -> CacheBinding {
        match ctx {
            CacheCtx::Fetch => CacheBinding {
                env: self.fetch_env.clone(),
                mounts: vec![format!("{warm_vol}:{}", self.dep_cache_path)],
            },
            CacheCtx::Lsp => CacheBinding {
                env: self.lsp_env.clone(),
                mounts: vec![format!("{warm_vol}:{}:ro", self.dep_cache_path)],
            },
            CacheCtx::Verify => CacheBinding {
                env: self.verify_env.clone(),
                mounts: vec![format!("{verify_vol}:{}", self.verify_cache_path)],
            },
        }
    }
}

/// The hardcoded Rust profile — reproduces today's three cargo sites exactly (Step 1).
pub fn rust_profile() -> LanguageProfile {
    LanguageProfile {
        id: "rust".to_string(),
        fetch_cmd: "cargo fetch --locked".to_string(),
        warm_cache_base: "a2a-impl-lsp-cache".to_string(),
        dep_cache_path: "/cargo".to_string(),
        verify_cache_path: "/cache".to_string(),
        fetch_env: vec![("CARGO_HOME".to_string(), "/cargo".to_string())],
        lsp_env: vec![], // Step 1: lsp env stays config-side (the agent MCP env).
        verify_env: vec![
            ("CARGO_HOME".to_string(), "/cache/cargo".to_string()),
            ("CARGO_TARGET_DIR".to_string(), "/cache/target".to_string()),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_fetch_binding_matches_today() {
        let p = rust_profile();
        let b = p.cache_binding(CacheCtx::Fetch, "warmvol", "verifyvol");
        assert_eq!(
            b.env,
            vec![("CARGO_HOME".to_string(), "/cargo".to_string())]
        );
        assert_eq!(b.mounts, vec!["warmvol:/cargo".to_string()]);
    }

    #[test]
    fn rust_lsp_binding_is_ro_mount_no_env() {
        // Step 1: the lsp runtime ENV stays in config (the agent MCP env); the seam owns only the MOUNT.
        let p = rust_profile();
        let b = p.cache_binding(CacheCtx::Lsp, "warmvol", "verifyvol");
        assert!(b.env.is_empty(), "lsp env stays config-side in Step 1");
        assert_eq!(b.mounts, vec!["warmvol:/cargo:ro".to_string()]);
    }

    #[test]
    fn rust_verify_binding_matches_today() {
        let p = rust_profile();
        let b = p.cache_binding(CacheCtx::Verify, "warmvol", "verifyvol");
        assert_eq!(
            b.env,
            vec![
                ("CARGO_HOME".to_string(), "/cache/cargo".to_string()),
                ("CARGO_TARGET_DIR".to_string(), "/cache/target".to_string()),
            ]
        );
        assert_eq!(b.mounts, vec!["verifyvol:/cache".to_string()]);
    }

    #[test]
    fn rust_fetch_cmd_is_cargo_fetch_locked() {
        assert_eq!(rust_profile().fetch_cmd, "cargo fetch --locked");
        assert_eq!(rust_profile().warm_cache_base, "a2a-impl-lsp-cache");
    }
}
