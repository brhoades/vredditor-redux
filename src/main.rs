use std::env;
use std::net::SocketAddr;
use std::sync::Arc;

use failure::{format_err, Error};
use log::{debug, error, info, warn};
use url::Url;

use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Mutex},
};
use tokio_tungstenite::tungstenite::protocol::Message;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = env_logger::try_init();
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:8080".to_string());
    let q = JobQueue::new();

    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    info!("Listening on: {}", addr);

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(accept_connection(q.sender(), stream));
    }

    Ok(())
}

#[derive(Clone, Debug)]
enum StreamState {
    New,
    Handshake,
    Url(Url),
    Job,
    Invalid(String),
}

impl StreamState {
    pub fn to_string(&self) -> String {
        use StreamState::*;
        match self {
            New => "NEW".to_string(),
            Handshake => "HANDSHAKE".to_string(),
            Url(v) => "URL v".to_string(),
            Job => "JOB".to_string(),
            Invalid(v) => format!("Unknown command: {}", v),
        }
    }
}

impl std::fmt::Display for StreamState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

impl<T> From<T> for StreamState
where
    T: AsRef<str>,
{
    fn from(s: T) -> Self {
        use StreamState::*;

        match s.as_ref() {
            "NEW" => New,
            "HANDSHAKE" => Handshake,
            "JOB" => Job,
            v => {
                if v.starts_with("URL ") {
                    match url::Url::parse(v.trim_start_matches("URL ")) {
                        Ok(v) => Url(v),
                        Err(e) => Invalid(format!("bad url: {}", e)),
                    }
                } else {
                    Invalid(v.to_string())
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum JobState {
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

#[derive(Debug)]
pub struct Job {
    pub state: Arc<Mutex<JobState>>,
    pub url: Url,
}

async fn accept_connection(send_job: mpsc::Sender<Job>, stream: TcpStream) -> Result<(), Error> {
    let addr = stream
        .peer_addr()
        .map_err(|e| format_err!("connected streams should have a peer address: {}", e))?;
    info!("Peer address: {}", addr);

    let mut stream = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|e| format_err!("error during handshake: {}", e))?;
    let (mut tx, mut rx) = stream.split();

    let mut stream_state = StreamState::New;

    while let Some(msg) = rx.next().await {
        if let Err(e) = msg {
            warn!("{} -> error on connection: {}", addr, e);
            continue;
        }

        let msg = match msg.unwrap() {
            Message::Text(v) => {
                debug!("{} -> {:?}", addr, v);
                v
            }
            v => {
                info!("{} -> unknown message type: {:?}", addr, v);
                tx.send(Message::Text(format!("unknown proto message type: {}", v)))
                    .await?;
                continue;
            }
        };

        match handle_message(&stream_state, &mut tx, &addr, msg).await {
            Ok(state) => stream_state = state,
            Err(e) => {
                warn!(
                    "{} -> handle_message errored, assuming conn closed: {}",
                    addr, e
                );

                return Ok(());
            }
        }
    }
    Ok(())
}

async fn handle_message<T>(
    state: &StreamState,
    tx: &mut T,
    addr: &SocketAddr,
    msg: String,
) -> Result<StreamState, Error>
where
    T: futures::Sink<Message> + std::marker::Unpin,
    <T as futures::Sink<Message>>::Error: std::error::Error + Sync + Send + failure::Fail + 'static,
{
    match StreamState::from(msg) {
        StreamState::New => {
            debug!("{} -> tried to reset already active connection", addr);
            Ok(state.clone())
        }
        StreamState::Handshake => match state {
            New => tx
                .send(Message::Text(format!("READY")))
                .await
                .map(|_| StreamState::Handshake),
            other => {
                debug!("{} -> unexpected handshake", addr);
                tx.send(Message::Text(format!("cannot handshake at this time")))
                    .await
                    .map(|_| state.clone())
            }
        },
        StreamState::Url(url) => match state {
            StreamState::Handshake => {
                debug!("{} -> enqueuing URL {}", addr, url);
                Ok(StreamState::Job)
            }
            other => {
                debug!("{} -> unexpected url", addr);
                tx.send(Message::Text(format!("cannot process url at this time")))
                    .await
                    .map(|_| state.clone())
            }
        },
        StreamState::Job => Ok(StreamState::Handshake),
        StreamState::Invalid(v) => {
            debug!("{} -> Invalid message \"{}\"", addr, v);
            tx.send(Message::Text(format!("unknown proto message type: {}", v)))
                .await
                .map(|_| state.clone())
        }
    }
    .map_err(|e| format_err!("{}", e)) // TODO: the trait should handle this. I need more type bounds.
}

#[derive(Debug)]
struct JobQueue {
    rx: mpsc::Receiver<Job>,
    tx: mpsc::Sender<Job>,
}

impl JobQueue {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Job>(100);
        JobQueue { tx, rx }
    }

    pub fn sender(&self) -> mpsc::Sender<Job> {
        self.tx.clone()
    }

    pub async fn run(self) -> () {
        let mut stream = self.rx.map(process_job);

        while let Some(res) = stream.next().await {
            println!("res: {:?}", res.await);
        }
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
