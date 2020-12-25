use anyhow::*;
use lib::run_server;

#[tokio::main]
async fn main() -> Result<()> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:8080".to_string());
    run_server(addr).await
}
