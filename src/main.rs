use std::env;
use std::net::SocketAddr;
use std::sync::Arc;

use failure::{format_err, Error};
use log::{debug, error, info, warn};
use url::Url;

use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, Mutex},
    try_join,
};
use tokio_tungstenite::tungstenite::protocol::Message;

type Result<T> = std::result::Result<T, Error>;

#[tokio::main]
async fn main() -> Result<()> {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:8080".to_string());
    run_server(addr).await
}

async fn run_server(addr: String) -> Result<()> {
    let _ = env_logger::try_init();
    let q = JobQueue::new();
    let enqueuer = q.enqueuer();

    try_join!(q.run(), tcp_listener_loop(enqueuer, addr))?;
    Ok(())
}

async fn tcp_listener_loop(eq: JobEnqueuer, addr: String) -> Result<()> {
    // Create the event loop and TCP listener we'll accept connections on.
    let listener = TcpListener::bind(&addr).await?;
    info!("Listening on: {}", addr);

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(accept_connection(eq.clone(), stream));
    }

    Ok(())
}

#[derive(Clone, Debug)]
enum StreamState {
    New,
    Handshake,
    Url(Url),
    Job(Job),
    Invalid(String),
}

impl Into<String> for StreamState {
    fn into(self) -> String {
        match self {
            StreamState::New => "NEW".to_string(),
            StreamState::Handshake => "HANDSHAKE".to_string(),
            StreamState::Url(v) => format!("URL {}", v),
            StreamState::Job(_job) => "JOB".to_string(),
            StreamState::Invalid(v) => format!("Unknown command: {}", v),
        }
    }
}

impl std::fmt::Display for StreamState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let string = match self {
            StreamState::New => "NEW".to_string(),
            StreamState::Handshake => "HANDSHAKE".to_string(),
            StreamState::Url(v) => format!("URL {}", v),
            StreamState::Job(_job) => "JOB".to_string(),
            StreamState::Invalid(v) => format!("Unknown command: {}", v),
        };
        write!(f, "{}", string)
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
            "JOB" => Invalid("job cannot be retrieved as a proto message".to_string()),
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
    Complete(Result<url::Url>),
    Cancelled,
}

impl Default for JobState {
    fn default() -> Self {
        JobState::Queued
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub state: Arc<Mutex<JobState>>,
    pub url: Url,
}

async fn accept_connection(mut eq: JobEnqueuer, stream: TcpStream) -> Result<()> {
    let addr = stream
        .peer_addr()
        .map_err(|e| format_err!("connected streams should have a peer address: {}", e))?;
    info!("Peer address: {}", addr);

    let stream = tokio_tungstenite::accept_async(stream)
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

        stream_state = match handle_message(&stream_state, &mut tx, &mut eq, &addr, msg).await {
            Ok(state) => state,
            Err(e) => {
                warn!(
                    "{} -> handle_message errored, assuming conn closed: {}",
                    addr, e
                );

                return Ok(());
            }
        };
    }
    Ok(())
}

async fn handle_message<T>(
    state: &StreamState,
    tx: &mut T,
    eq: &mut JobEnqueuer,
    addr: &SocketAddr,
    msg: String,
) -> Result<StreamState>
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
            StreamState::New => tx
                .send(Message::Text("READY".to_string()))
                .await
                .map(|_| StreamState::Handshake),
            other => {
                debug!("{} -> unexpected handshake from state {}", addr, other);
                tx.send(Message::Text("cannot handshake at this time".to_string()))
                    .await
                    .map(|_| state.clone())
            }
        },
        StreamState::Url(url) => match state {
            StreamState::Handshake => {
                debug!("{} -> enqueuing URL {}", addr, url);
                let job = eq.enqueue_url(url).await?;
                debug!("{} -> job created, url enqueued: {:?}", addr, job);

                tx.send(Message::Text("QUEUED".to_string()))
                    .await
                    .map(|_| StreamState::Job(job))
            }
            other => {
                debug!(
                    "{} -> tried to provide url at state {} (url: {})",
                    addr, other, other,
                );
                tx.send(Message::Text("cannot process url at this time".to_string()))
                    .await
                    .map(|_| state.clone())
            }
        },
        StreamState::Job(_job) => Ok(StreamState::Handshake),
        StreamState::Invalid(v) => {
            debug!("{} -> Invalid message \"{}\"", addr, v);
            tx.send(Message::Text(format!("unknown proto message type: {}", v)))
                .await
                .map(|_| state.clone())
        }
    }
    .map_err(failure::Error::from)
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

    pub fn enqueuer(&self) -> JobEnqueuer {
        JobEnqueuer {
            tx: self.tx.clone(),
        }
    }

    pub async fn run(self) -> Result<()> {
        let mut stream = self.rx.map(process_job);

        while let Some(res) = stream.next().await {
            println!("res: {:?}", res.await);
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct JobEnqueuer {
    tx: mpsc::Sender<Job>,
}

impl JobEnqueuer {
    #[allow(dead_code)]
    pub async fn enqueue<T: AsRef<str>>(&mut self, url: T) -> Result<Job> {
        let j = Job {
            url: Url::parse(url.as_ref())?,
            state: Arc::new(Mutex::new(JobState::default())),
        };

        self.tx.send(j.clone()).await?;
        Ok(j)
    }

    pub async fn enqueue_url(&mut self, url: Url) -> Result<Job> {
        let j = Job {
            url,
            state: Arc::new(Mutex::new(JobState::default())),
        };

        self.tx.send(j.clone()).await?;
        Ok(j)
    }
}

async fn process_job(job: Job) {
    let mut state = job.state.lock().await;
    match &*state {
        JobState::Queued => (),
        // TODO: log
        v => {
            error!("process job got job with state {:?}", v);
            return;
        }
    }
    *state = JobState::Processing;
    std::mem::drop(state); // allow ws access
}

#[tokio::test]
async fn test_client() -> Result<()> {
    use tokio::select;
    let addr = "localhost:50023".to_owned();

    // we cancel by returning an error from any ne of these
    select! {
        res = run_server(addr.clone()) => {
            return res;
        },
        res = run_test_client(addr) => {
            return res;
        },
    }
}

#[cfg(test)]
async fn run_test_client(addr: String) -> Result<()> {
    use std::time::Duration;
    use tokio::time::sleep;
    use websocket_lite::Message;

    let mut tries: i8 = 0;

    // just keeping this in a loop so I don't have to type the return value of ClientBuilder.
    while tries < 5 {
        // wait for the server to come up.
        sleep(Duration::from_millis(50)).await;

        let url = format!("ws://{}", addr);
        let stream = websocket_lite::ClientBuilder::new(&url)?
            .async_connect()
            .await
            .map_err(|e| format_err!("failed to connect to ws server @ {}: {}", addr, e));
        let mut stream = match stream {
            Ok(s) => s,
            Err(e) => {
                if tries >= 4 {
                    return Err(e);
                } else {
                    tries += 1;
                    continue;
                }
            }
        };

        assert_eq!(
            "READY",
            send_and_get(&mut stream, Message::text(StreamState::Handshake)).await?
        );

        assert_eq!(
            "QUEUED",
            send_and_get(
                &mut stream,
                Message::text(StreamState::Url(Url::parse("https://google.com")?))
            )
            .await?
        );

        break;
    }

    Ok(())
}

#[cfg(test)]
async fn send_and_get<T>(stream: &mut T, msg: websocket_lite::Message) -> Result<String>
where
    T: futures::Sink<websocket_lite::Message>
        + futures::Stream<Item = std::result::Result<websocket_lite::Message, websocket_lite::Error>>
        + Unpin,
    <T as futures::Sink<websocket_lite::Message>>::Error: std::fmt::Display,
{
    let msg_text = msg.as_text().unwrap();

    stream
        .send(msg.clone())
        .await
        .map_err(|e| format_err!("failed to send message \"{}\" from client: {}", msg_text, e))?;

    stream
        .next()
        .await
        .ok_or_else(|| {
            format_err!(
                "no message received from server in response to: \"{}\"",
                msg_text
            )
        })?
        .map_err(|e| {
            format_err!(
                "error from server in response to message \"{}\": {}",
                msg_text,
                e
            )
        })
        .and_then(|v| {
            v.as_text()
                .ok_or_else(|| {
                    format_err!(
                        "no message received from server in response to message: {}",
                        msg_text
                    )
                })
                .map(str::to_string)
        })
}
