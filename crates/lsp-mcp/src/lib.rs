//! lsp-mcp: an LSP-over-MCP shim. Wraps a language server (rust-analyzer in Slice A) and exposes
//! type-resolved navigation as MCP tools. See docs/superpowers/specs/2026-06-13-lsp-mcp-nav-design.md.
use std::path::PathBuf;

pub mod cache_key;
pub mod lang;
pub mod lsp;
pub mod mcp;
pub mod shape;

#[derive(clap::Parser, Debug)]
#[command(name = "lsp-mcp")]
pub struct Cli {
    /// Repo root the language server is rooted at (the session cwd).
    #[arg(long)]
    pub repo: PathBuf,
    /// Language server to drive. Slice A supports only "rust".
    #[arg(long, default_value = "rust")]
    pub lang: String,
    /// Base dir for the per-repo shared build cache (CARGO_TARGET_DIR). Optional.
    #[arg(long)]
    pub target_cache: Option<PathBuf>,
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    anyhow::ensure!(
        cli.lang == "rust",
        "Slice A supports only --lang rust (got {:?})",
        cli.lang
    );
    let repo = cli
        .repo
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("repo {:?}: {e}", cli.repo))?;
    anyhow::ensure!(
        repo.join("Cargo.toml").exists(),
        "not a cargo repo (no Cargo.toml): {:?}",
        repo
    );
    let target = cli.target_cache.as_deref().map(|base| {
        let origin = git_origin(&repo);
        cache_key::cache_dir(base, &repo, origin.as_deref())
    });
    let session = lsp::LspClient::start_with(&repo, lang::rust_ra_config(target.as_deref()))?;
    mcp::serve(session)
}

fn git_origin(repo: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[doc(hidden)] // out of the documented API, but resolvable from the external characterization harness.
pub mod testkit {
    //! Internal helpers exposed ONLY for the characterization harness (tests/characterization.rs).
    //! Doc-hidden so they don't appear in the public docs; the items themselves are `pub` because an
    //! external `tests/` crate cannot reach `pub(crate)` items (and `pub use` of `pub(crate)` won't compile).
    pub use crate::lang::{PyrightReady, Readiness, RustReady};
}
