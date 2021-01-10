#![feature(async_closure)]
#![feature(or_patterns)]

mod file;
pub(crate) mod internal;
pub mod proto;
pub mod s3;
mod token;
pub mod ytdl;

use std::convert::TryFrom;
use std::net::SocketAddr;
use std::sync::Arc;

use proto::*;
use url::Url;

use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, Mutex},
    try_join,
};
use tokio_tungstenite::tungstenite::protocol::Message;

use internal::*;

pub async fn run_server(addr: String) -> Result<()> {
    if let Err(e) = pretty_env_logger::try_init() {
        eprintln!("failed to initialize env_logger: {}", e);
    }

    let q = JobQueue::new();
    let enqueuer = q.enqueuer();
    let authz = token::EnvAuthorizer::new_from_env("VR_AUTHORIZED_KEYS")?;
    try_join!(q.run(), tcp_listener_loop(enqueuer, authz, addr))?;
    Ok(())
}

async fn tcp_listener_loop<A: token::Authorizer + 'static>(
    eq: JobEnqueuer,
    authz: A,
    addr: String,
) -> Result<()> {
    // Create the event loop and TCP listener we'll accept connections on.
    let listener = TcpListener::bind(&addr).await?;
    info!("Listening on: {}", addr);

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(accept_connection(eq.clone(), authz.clone(), stream));
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
            (state, None) => {
                *state = new;
                true
            }
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
        }
    }
}

#[derive(Debug, Clone)]
enum StreamState {
    New,
    Waiting,
    Running(Job),
    Closed,
}

impl StreamState {
    // Returns a job's state if one is running, else NoJobs.
    async fn job_state(&self) -> Option<JobState> {
        match &self {
            StreamState::Running(j) => match j.state.lock().await.clone() {
                JobState::NoJobs(_) => None,
                JobState::Unknown(_) => None,
                other => Some(other),
            },
            _ => None,
        }
    }

    /// errors if the state isn't auth'd.
    /// XXX: deprecated: when streamstate can be updated by a subscriber,
    /// this can be replated by a "Completed" state for jobs.
    async fn bail_if_unauthed(&self) -> Result<(), Error> {
        match &self {
            StreamState::New => Err(format_err!("unauthenticated: cannot use function")),
            _ => Ok(()),
        }
    }
}

async fn accept_connection<A: token::Authorizer>(
    mut eq: JobEnqueuer,
    authz: A,
    stream: TcpStream,
) -> Result<()> {
    let addr = stream
        .peer_addr()
        .context("connected streams should have a peer address")?;
    info!("Peer address: {}", addr);

    let stream = tokio_tungstenite::accept_async(stream)
        .await
        .context("error during handshake")?;
    let (tx, rx) = stream.split();

    let mut stream_state = StreamState::New;
    let mut rx = rx
        .map(|v| match v {
            Err(e) => panic!(e), // infallible
            Ok(o) => o,
        })
        // XXX: handle `Ping` from tungstenite.
        //   error on connection: unsupported transcode request type from client: Ping([80, 73, 78, 71])
        .map(|v| ClientMessage::try_from(v));
    let mut tx = tx.with(|resp| futures::future::ok::<_, Error>(Into::<Message>::into(resp)));

    while let Some(msg) = rx.next().await {
        // XXX: hack, until we subscribe, if we're queued update the job state before processing.
        stream_state = match msg {
            Ok(msg) => {
                match handle_message(stream_state, &mut tx, authz.clone(), &mut eq, &addr, msg)
                    .await
                {
                    Ok(state) => state,
                    Err(e) => {
                        warn!(
                            "{} -> handle_message errored, assuming conn closed: {}",
                            addr, e
                        );

                        return Ok(());
                    }
                }
            }
            Err(e) => {
                warn!("{} -> error on connection: {}", addr, e);
                tx.send(ServerMessage::error(format!("protocol error: {}", e)))
                    .await?;
                continue;
            }
        };
    }
    Ok(())
}

async fn handle_message<'a, T, A: token::Authorizer>(
    state: StreamState,
    tx: &mut T,
    authz: A,
    eq: &mut JobEnqueuer,
    addr: &SocketAddr,
    msg: ClientMessage,
) -> Result<StreamState>
where
    T: futures::Sink<ServerMessage, Error = Error>
        + futures::SinkExt<ServerMessage>
        + std::marker::Unpin,
{
    use ClientMessage::*;

    match (state.job_state().await, msg) {
        (None, Transcode(TranscodeReqMessage::Handshake(Handshake { ref token }))) => {
            match token.as_ref().and_then(|t| t.token.as_ref()) {
                Some(inner_token) => {
                    return if !authz.is_authorized(&inner_token) {
                        debug!("{} -> invalid handshake token: {:?}", addr, token);
                        tx.send(ServerMessage::handshake_rejected("token is not authorized"))
                            .await?;
                        Ok(StreamState::New)
                    } else {
                        debug!("{} -> handshake accepted", addr);
                        tx.send(ServerMessage::handshake_accepted("READY")).await?;
                        Ok(StreamState::Waiting)
                    }
                }
                None => {
                    debug!(
                        "{} -> token is required but not provided: {:?}",
                        addr, token
                    );
                    tx.send(ServerMessage::handshake_rejected(
                        "token is required but not provided",
                    ))
                    .await?;
                }
            }
        }
        (Some(state), Transcode(TranscodeReqMessage::Handshake(_))) => {
            debug!(
                "{} -> unexpected handshake from non-new state: {:?}",
                addr, state,
            );
            tx.send(ServerMessage::handshake_rejected(
                "cannot handshake at this time",
            ))
            .await?;
        }
        (
            Some(JobState::Completed(_)),
            Transcode(TranscodeReqMessage::Transcode(TranscodeOpts { url })),
        )
        | (None, Transcode(TranscodeReqMessage::Transcode(TranscodeOpts { url }))) => {
            state.bail_if_unauthed().await?;
            match &state {
                StreamState::New => unreachable!("checked above"),
                StreamState::Running(j) => {
                    error!("hit unexpected running + none state");
                    match *j.state.lock().await {
                        JobState::Unknown(_) | JobState::NoJobs(_) | JobState::Completed(_) => (),
                        _ => {
                            tx.send(ServerMessage::error(
                                "there is a job queued. wait for it to finish or cancel it before starting another",
                            ))
                                .await?;
                        }
                    }
                }
                StreamState::Closed => {
                    warn!("{} -> enqueue after stream closed", addr);
                    tx.send(ServerMessage::close_with_reason(
                        CloseCode::Error,
                        "connection already closed".to_string(),
                    ))
                    .await?;
                }
                StreamState::Waiting => (),
            }

            let url = match Url::parse(&url) {
                Ok(url) => url,
                Err(e) => {
                    tx.send(ServerMessage::error(format!("error: {}", e)))
                        .await?;
                    return Ok(state);
                }
            };
            debug!("{} -> enqueuing URL {}", addr, url);

            let job = eq.enqueue_url(url).await?;
            debug!("{} -> job created, url enqueued: {:?}", addr, job);

            tx.send(ServerMessage::queued()).await?;
            return Ok(StreamState::Running(job));
        }
        (Some(job_state), Transcode(TranscodeReqMessage::Transcode(_))) => {
            state.bail_if_unauthed().await?;
            debug!(
                "{} -> attempted to enqueue from invalid state ({:?})",
                addr, job_state,
            );
            tx.send(ServerMessage::error(
                "there is a job queued. wait for it to finish or cancel it before starting another",
            ))
            .await?;
        }
        (Some(job_state), Transcode(TranscodeReqMessage::Status(_))) => {
            state.bail_if_unauthed().await?;
            debug!("{} -> job status", addr);
            debug!("{} <- {}", addr, &job_state);
            tx.send(ServerMessage::job_status(job_state)).await?;
        }
        (None, Transcode(TranscodeReqMessage::Status(_))) => {
            state.bail_if_unauthed().await?;
            // special derived statuses
            debug!("{} -> job status without job", addr);
            match &state {
                StreamState::New => {
                    tx.send(ServerMessage::close_with_reason(
                        CloseCode::Protocol,
                        "must successfully handshake prior to job status commands".to_string(),
                    ))
                    .await?;
                    return Ok(StreamState::Closed);
                }
                StreamState::Waiting => {
                    tx.send(ServerMessage::job_status(JobState::NoJobs(())))
                        .await?
                }
                StreamState::Running(_) => unreachable!("handled by above case"),
                StreamState::Closed => (),
            };
        }
        (Some(JobState::Completed(_)) | None, Transcode(TranscodeReqMessage::Cancel(_))) => {
            state.bail_if_unauthed().await?;
            debug!("{} -> cancel job", addr);
            tx.send(ServerMessage::error("cancel is not possible at this time"))
                .await?;
            debug!("{} <- cannot cancel from state", addr);
        }
        (Some(_), Transcode(TranscodeReqMessage::Cancel(_))) => {
            state.bail_if_unauthed().await?;
            debug!("{} -> cancel job", addr);

            // signal to the running job that it's done, then return a new state
            // XXX: this likely does nothing
            if let StreamState::Running(mut job) = state {
                job.set_state_if_eq(JobState::Cancelled(()), None).await;
            } else {
                unreachable!("shoudn't happen: {:?}", state);
            }

            tx.send(ServerMessage::job_status(JobState::Cancelled(())))
                .await?;
            return Ok(StreamState::Waiting);
        }
        (_, Ping(payload)) => {
            debug!("{} -> ping", addr);
            tx.send(ServerMessage::pong(payload)).await?;
            debug!("{} <- pong", addr);
        }
        (_, CloseReason { reason, code }) => {
            debug!("{} -> closed with code {}: {}", addr, code, reason);
            return Ok(StreamState::Closed);
        }
        (_, Close) => {
            debug!("{} -> close without reason", addr);
            return Ok(StreamState::Closed);
        }
        (_, Unsupported(msg)) => {
            error!("{} -> unsupported message\n{:?}", addr, msg);
        }
    }

    Ok(state)
}

#[derive(Debug)]
pub struct JobQueue {
    rx: mpsc::Receiver<Job>,
    tx: mpsc::Sender<Job>,
}

impl JobQueue {
    // Builds a job queue with required dependencies to execute jobs:
    //   Shared s3 client form the default AWS_REGION environment variable.
    //   Shared reqwest client to use its connection pooling / http2 benefits.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Job>(100);
        JobQueue { tx, rx }
    }

    fn enqueuer(&self) -> JobEnqueuer {
        JobEnqueuer {
            tx: self.tx.clone(),
        }
    }

    // XXX: runs single threaded, maybe it shouldn't? A few pooled runners makes sense.
    pub async fn run(mut self) -> Result<()> {
        // XXX: this doesn't work after tokio 1.0 as receviers are no longer streams?
        // use tokio_stream::StreamExt;

        while let Some(res) = self.rx.recv().await {
            println!("job processed: {:?}", process_job(res).await);
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

    match handle_job(&mut job).await {
        Ok(uploaded) => {
            info!("completed job for {} to {}", job.url, uploaded);
            job.set_state_if_eq(JobState::Completed(Ok(uploaded).into()), None)
                .await;
        }
        Err(e) => {
            warn!(
                "error processing job {:?} for {}:\n{}\n{}",
                job,
                &job.url,
                e,
                e.chain()
                    .into_iter()
                    .map(|e| format!("{}", e))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            job.set_state_if_eq(
                JobState::Completed(RawProtoResultWrapper::from(Err::<String, _>(e))),
                None,
            )
            .await;
        }
    };
}

// XXX: move to a setting
static BUCKET: &'static str = "i.brod.es";
static ACL: &'static str = "public-read";

async fn handle_job(job: &mut Job) -> Result<String> {
    let clients: Clients = Default::default();
    let uploader: s3::S3Uploader<bytes::BytesMut, file::TempFileStream> =
        s3::S3Uploader::<bytes::BytesMut, file::TempFileStream>::default()
            .bucket(BUCKET)
            .acl(ACL)
            .clone();

    let url = &job.url;
    let s3_client = &clients.s3_client;
    let mut file = ytdl::youtube_dl_download(&clients, url, Option::<&[&str]>::None).await?;
    debug!(
        "downloaded {} bytes",
        file.file_mut()
            .metadata()
            .await
            .map(|v| v.len().to_string())
            .unwrap_or_else(|_| format!(
                "[couldn't query bytes for {} in {}]",
                url,
                file.filename().unwrap_or("unknown".to_owned())
            ))
    );
    job.set_state_if_eq(
        JobState::Uploading(Default::default()),
        Some(JobState::Processing(Default::default())),
    )
    .await;

    let mut uploader = uploader.clone();
    let filename = format!("{}.{}", file.filename()?, "mp4");
    uploader
        .filename(&filename)
        .content_length(file.len().await.context("len for upload to s3")? as i64)
        .tag("app", "vredditor")
        .data(file.stream().await.context("failed to get file stream")?);

    uploader
        .upload(s3_client)
        .await
        .map(|_| format!("https://{}/{}", BUCKET, filename))
        .with_context(|| format!("failed to upload file from {} to s3", job.url))
}
