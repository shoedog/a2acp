//! lsp-mcp: an LSP-over-MCP shim. Wraps a language server (rust-analyzer in Slice A) and exposes
//! type-resolved navigation as MCP tools. See docs/superpowers/specs/2026-06-13-lsp-mcp-nav-design.md.
use std::path::PathBuf;

pub mod cache_key;
pub mod lsp;
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
    // mcp::serve(session) — implemented in later tasks
    let _ = cli;
    Ok(())
}
