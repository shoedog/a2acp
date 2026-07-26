#[path = "../acp/attested_wrapper.rs"]
mod attested_wrapper;

#[tokio::main]
async fn main() {
    if let Err(error) = attested_wrapper::run_from_env().await {
        eprintln!("codex-acp-attested: {error}");
        std::process::exit(1);
    }
}
