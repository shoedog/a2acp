fn main() -> anyhow::Result<()> {
    let cli = <lsp_mcp::Cli as clap::Parser>::parse();
    lsp_mcp::run(cli)
}
