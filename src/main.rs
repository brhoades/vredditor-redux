use failure::{format_err, Error};
use std::env;

use futures_util::StreamExt;
use log::info;
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = env_logger::try_init();
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:8080".to_string());

    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    info!("Listening on: {}", addr);

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(accept_connection(stream));
    }

    Ok(())
}

async fn accept_connection(stream: TcpStream) -> Result<(), Error> {
    let addr = stream
        .peer_addr()
        .map_err(|e| format_err!("connected streams should have a peer address: {}", e))?;
    info!("Peer address: {}", addr);

    let ws_stream = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|e| format_err!("error during handshake: {}", e))?;

    info!("New WebSocket connection: {}", addr);

    let (write, read) = ws_stream.split();
    read.forward(write)
        .await
        .map_err(|e| format_err!("failed to echo: {}", e))
}
