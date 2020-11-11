use std::env;
use std::sync::Arc;

use failure::{format_err, Error};
use log::{debug, error, info};
use url::Url;

use futures::{
    channel::mpsc::{channel, Receiver, Sender},
    StreamExt,
};

use tokio::{
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

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

#[derive(Debug)]
struct JobQueue {
    rx: Receiver<Job>,
    tx: Sender<Job>,
}

impl JobQueue {
    pub fn new() -> Self {
        let (tx, rx) = channel::<Job>(100);
        JobQueue { tx, rx }
    }

    pub async fn sender(&self) -> Sender<Job> {
        self.tx.clone()
    }

    pub async fn run(self) -> () {
        self.rx.for_each(|job| process_job(job)).await;
    }

    fn enqueue<T: AsRef<str>>(url: T) -> Result<Job, Error> {
        Ok(Job {
            url: Url::parse(url.as_ref())?,
            state: Arc::new(Mutex::new(JobState::default())),
        })
    }
}

async fn process_job(job: Job) {
    use JobState::*;

    let mut state = job.state.lock().await;
    match &*state {
        Queued => (),
        // TODO: log
        v => {
            error!("process job got job with state {:?}", v);
            return;
        }
    }
    *state = Processing;
    std::mem::drop(state); // allow ws access
}

#[derive(Debug)]
struct Job {
    pub state: Arc<Mutex<JobState>>,
    pub url: Url,
}

#[derive(Debug)]
enum JobState {
    Queued,
    Processing,
    Uploading,
    Complete(Result<url::Url, Error>),
    Cancelled,
}

impl Default for JobState {
    fn default() -> Self {
        JobState::Queued
    }
}
