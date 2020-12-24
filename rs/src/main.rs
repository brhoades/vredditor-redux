pub mod proto;

use std::convert::{TryFrom, TryInto};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::*;
use log::{debug, error, info, warn};
use proto::*;
use url::Url;

use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{mpsc, Mutex},
    try_join,
};
use tokio_tungstenite::tungstenite::protocol::Message;

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

#[derive(Debug, Clone)]
pub struct Job {
    pub state: Arc<Mutex<JobState>>,
    pub url: Url,
}

impl Job {
    // Atomically sets the state if the expected state is there and returns if it was successful.
    async fn set_state_if_eq(&mut self, new: JobState, expected: Option<JobState>) -> bool {
        let mut state = self.state.lock().await;
        match (&mut *state, expected) {
            (state, Some(expected)) => {
                if *state == expected {
                    *state = new;
                    true
                } else {
                    error!(
                        "process_job called with job in bad state {:?} (expected {})",
                        state, expected
                    );
                    *state = JobState::Completed(
                        Err::<String, _>(format_err!("internal server error (state mismatch)"))
                            .into(),
                    );
                    false
                }
            }
            (state, None) => {
                *state = new;
                true
            }
        }
    }
}

#[derive(Debug, Clone)]
enum StreamState {
    New,
    Handshake,
    Queued(Job),
    Done(Job),
}

async fn accept_connection(mut eq: JobEnqueuer, stream: TcpStream) -> Result<()> {
    let addr = stream
        .peer_addr()
        .context("connected streams should have a peer address")?;
    info!("Peer address: {}", addr);

    let stream = tokio_tungstenite::accept_async(stream)
        .await
        .context("error during handshake")?;
    let (mut tx, rx) = stream.split();

    let mut stream_state = StreamState::New;
    let mut rx = rx.map(|v| match v {
        Err(e) => panic!("infallible"),
        Ok(o) => o,
    }).map(|v| match TranscodeReq::try_from(v) {
        Ok(v) => v.req.ok_or_else(|| format_err!("missing transcode request message")),
        Err(v) => Err(v),
    });

    while let Some(msg) = rx.next().await {
        stream_state = match msg {
            Ok(msg) => match handle_message(stream_state, &mut tx, &mut eq, &addr, msg).await {
                Ok(state) => state,
                Err(e) => {
                    warn!(
                        "{} -> handle_message errored, assuming conn closed: {}",
                        addr, e
                    );

                    return Ok(());
                }
            },
            Err(e) => {
                warn!("{} -> error on connection: {}", addr, e);
                continue;
            },
        };
    }
    Ok(())
}

async fn handle_message<'a, T>(
    state: StreamState,
    tx: &mut T,
    eq: &mut JobEnqueuer,
    addr: &SocketAddr,
    msg: TranscodeMessage,
) -> Result<StreamState>
where
    T: futures::Sink<Message> + std::marker::Unpin,
    <T as futures::Sink<Message>>::Error: std::error::Error + Sync + Send + 'static,
{
    match msg {
        TranscodeMessage::Handshake(_) => match &state {
            StreamState::New => {
                tx.send(Message::Text("READY".to_string())).await?;
                return Ok(StreamState::Handshake);
            }
            other => {
                debug!("{} -> unexpected handshake from state {:?}", addr, other);
                tx.send(Message::Text("cannot handshake at this time".to_string()))
                    .await?;
            }
        },
        TranscodeMessage::Transcode(TranscodeOpts{
            url
        }) => match &state {
            StreamState::Handshake => {
                let url = match Url::parse(&url) {
                    Ok(url) => url,
                    Err(e) => {
                        tx.send(Message::Text(format!("error: {}", e))).await?;
                        return Ok(state);
                    },
                };
                debug!("{} -> enqueuing URL {}", addr, url);

                let job = eq.enqueue_url(url).await?;
                debug!("{} -> job created, url enqueued: {:?}", addr, job);

                tx.send(Message::Text("QUEUED".to_string())).await?;
                return Ok(StreamState::Queued(job));
            }
            other => {
                debug!(
                    "{} -> tried to provide url at state {:?}: {:?}",
                    addr, state, other,
                );
                tx.send(Message::Text("cannot process url at this time".to_string()))
                    .await?;
            }
        },
        TranscodeMessage::Status(_) => match &state {
            StreamState::Queued(job) | StreamState::Done(job) => {
                tx.send(Message::Text(format!("STATUS {}", job.state.lock().await)))
                    .await?
            }
            _ => tx.send(Message::Text("STATUS NO_JOBS".to_string())).await?,
        },
        v => {
            debug!("{} -> Invalid message \"{:?}\"", addr, v);
            tx.send(Message::Text(format!(
                "unknown proto message type: {:?}",
                v
            )))
            .await?;
        }
    }

    Ok(state)
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
            println!("job processed: {:?}", res.await);
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

async fn process_job(mut job: Job) {
    if !job
        .set_state_if_eq(
            JobState::Processing(Default::default()),
            Some(JobState::Queued(Default::default())),
        )
        .await
    {
        error!("process_job called with job in bad state: {:?}", job);
    }

    let url = &job.url;
    match youtube_dl_download(url.to_string()).await {
        Ok(bytes) => {
            println!("downloaded {} bytes", bytes.len());
            job.set_state_if_eq(
                JobState::Uploading(Default::default()),
                Some(JobState::Processing(Default::default())),
            )
            .await;
        }
        Err(e) => {
            warn!("error processing job {:?} for {}: {}", job, url, e);
            job.set_state_if_eq(
                JobState::Completed(RawProtoResultWrapper::from(Err::<String, _>(e))),
                Some(JobState::Processing(Default::default())),
            )
            .await;
        }
    }
}

fn youtube_dl<'a, S, I: IntoIterator<Item = S>>(url: S, extra_args: Option<I>) -> Box<Command>
where
    S: AsRef<std::ffi::OsStr>,
{
    let mut cmd = Box::new(Command::new("youtube-dl"));
    cmd.args(&["--max-filesize", "50m"]);

    match extra_args {
        Some(a) => {
            cmd.args(a);
        }
        None => (),
    };

    cmd.arg(url);
    cmd
}

async fn youtube_dl_download<S: AsRef<std::ffi::OsStr> + Clone>(url: S) -> Result<Vec<u8>> {
    let pathb = random_temp_file_path();
    let path = pathb
        .to_str()
        .ok_or_else(|| format_err!("failed to convert pathbuf to str: {:?}", pathb))?;
    debug!("temp file path created: {}", path);

    let template = format!("{}.%(ext)s", path);
    let filename_output = youtube_dl(url.clone(), Option::<Vec<S>>::None)
        .args(&["-q", "--get-filename", "-o"])
        .arg(&template)
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::piped())
        .spawn()?
        .wait_with_output()
        .await?;

    if !filename_output.status.success() {
        warn!(
            "failed to query file name for: {}\n{}",
            url.as_ref().to_str().unwrap(),
            std::str::from_utf8(&filename_output.stderr).map_err(|e| format_err!(
                "failed to parse stderr for filename output youtubedl: {}\n{:?}",
                e,
                filename_output.stderr
            ))?
        );
    }

    let filename = std::str::from_utf8(&filename_output.stdout)?.trim();
    info!("filename: {}", filename);

    let child = youtube_dl(url, Option::<Vec<S>>::None)
        .args(&["-q", "-o"])
        .arg(template)
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::piped())
        .spawn()?;

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format_err!("failed to run youtube-dl and get output: {}", e))?;

    if !output.status.success() {
        return Err(format_err!(
            "non-zero exit status ({}) from youtube-dl download:\n{}\n{}\n",
            output.status,
            std::str::from_utf8(&output.stderr)?,
            std::str::from_utf8(&output.stdout)?,
        ));
    }

    let res = match tokio::fs::read(&filename).await {
        std::result::Result::Ok(v) => {
            debug!("read {} bytes", v.len());
            Ok(v)
        }
        std::result::Result::Err(e) => Err(format_err!(
            "failed to read from youtube-dl output file \"{}\" (from {}): {}",
            filename,
            path,
            e,
        ))?,
    };
    std::fs::remove_file(filename).unwrap();
    res
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
            send_and_get(&mut stream, Message::text(TranscodeMessage::Handshake)).await?
        );

        assert_eq!(
            "QUEUED",
            send_and_get(
                &mut stream,
                Message::text(TranscodeMessage::Url(Url::parse(
                    "https://mobile.twitter.com/KatieDaviscourt/status/1317765993385529346"
                )?))
            )
            .await?
        );

        let mut int = tokio::time::interval(Duration::from_millis(100));
        let mut times: i8 = 0;
        while let _ = int.next().await {
            times += 1;
            if times >= 20 {
                break;
            }

            let res = send_and_get(&mut stream, Message::text(TranscodeMessage::Status)).await?;
            if res.starts_with("STATUS ") {
                debug!("status: \"{}\"", res);
                let res = res.trim_start_matches("STATUS ");

                if res.starts_with("PROCESSING") {
                    debug!("PROCESSING");
                } else if res.starts_with("UPLOADING") {
                    debug!("UPLOADING");
                    break;
                } else {
                    assert!(false, "unknown state: {}", res);
                }
            } else {
                assert_eq!("COMPLETE", res);
                debug!("COMPLETE: \"{}\"", res);
            }
        }

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

fn random_temp_file_path() -> std::path::PathBuf {
    use rand::Rng;

    let mut dir = env::temp_dir();
    let mut rng = rand::thread_rng();
    let chars: String = std::iter::repeat(())
        .map(|()| rng.sample(rand::distributions::Alphanumeric))
        .take(7)
        .collect();

    dir.push(chars.as_str());
    dir
}
