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
    /// Language server to drive: "auto" (detect from repo markers), "rust", "python", or "go".
    #[arg(long, default_value = "auto")]
    pub lang: String,
    /// Base dir for the per-repo shared build cache (CARGO_TARGET_DIR). Optional.
    #[arg(long)]
    pub target_cache: Option<PathBuf>,
    /// Python interpreter for basedpyright's `pythonPath` (highest-precedence override). Also LSP_MCP_PYTHON_PATH.
    #[arg(long)]
    pub python_path: Option<PathBuf>,
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    let repo = cli
        .repo
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("repo {:?}: {e}", cli.repo))?;
    // Track whether the language was chosen EXPLICITLY (vs auto-detected): an explicit `--lang rust|python`
    // is validated against the repo via `is_project_root` BEFORE starting (today explicit `--lang python`
    // on a non-Python dir is unguarded). `auto` is already validated by `detect_lang`.
    let explicit = cli.lang.as_str() != "auto";
    let lang = match cli.lang.as_str() {
        "auto" => crate::lang::detect_lang(&repo)?,
        "rust" => crate::lang::Lang::Rust,
        "python" => crate::lang::Lang::Python,
        "go" => crate::lang::Lang::Go,
        other => anyhow::bail!("--lang must be auto|rust|python|go (got {other:?})"),
    };
    // Observability (spec §1): a misrouted {cwd} landing on the wrong language is now LOUD in the log.
    eprintln!("[lsp-mcp] root={} lang={}", repo.display(), lang.as_str());
    let cfg = match lang {
        crate::lang::Lang::Rust => {
            let target = cli.target_cache.as_deref().map(|base| {
                let origin = git_origin(&repo);
                cache_key::cache_dir(base, &repo, origin.as_deref())
            });
            crate::lang::rust_ra_config(target.as_deref())
        }
        crate::lang::Lang::Python => {
            crate::lang::pyright_config(&repo, cli.python_path.as_deref())?
        }
        crate::lang::Lang::Go => anyhow::bail!("go not yet implemented"), // TODO Task 4
    };
    // USE `is_project_root`: validate an EXPLICIT --lang against the repo (auto already validated above).
    if explicit && !(cfg.is_project_root)(&repo) {
        anyhow::bail!(
            "explicit --lang {} but {:?} is not a {} project root (missing the {} root markers); \
             pass the right --lang or point --repo at a {} root",
            lang.as_str(),
            repo,
            lang.as_str(),
            lang.as_str(),
            lang.as_str()
        );
    }
    let session = lsp::LspClient::start_with(&repo, cfg)?;
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
