#[path = "../acp/attested_wrapper.rs"]
mod attested_wrapper;

#[tokio::main]
async fn main() {
    let argv = std::env::args().skip(1).collect();
    if let Err(error) = attested_wrapper::run_from_args(argv).await {
        eprintln!("codex-acp-attested: {error}");
        std::process::exit(1);
    }
}
