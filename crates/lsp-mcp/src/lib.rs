//! lsp-mcp: an LSP-over-MCP shim. Wraps a language server (rust-analyzer in Slice A) and exposes
//! type-resolved navigation as MCP tools. See docs/superpowers/specs/2026-06-13-lsp-mcp-nav-design.md.
use std::path::PathBuf;

pub mod cache_key;
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
    let mut session = lsp::LspSession::start(&repo, target.as_deref())?;
    session.wait_ready(std::time::Duration::from_secs(30))?;
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
