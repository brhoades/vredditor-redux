#![feature(async_closure)]

mod file;
pub(crate) mod internal;
pub mod proto;
pub mod s3;
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
    Handshake,
    Queued(Job),
    #[allow(dead_code)]
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
    let (tx, rx) = stream.split();

    let mut stream_state = StreamState::New;
    let mut rx = rx
        .map(|v| match v {
            Err(e) => panic!(e), // infallible
            Ok(o) => o,
        })
        .map(|v| match TranscodeReq::try_from(v) {
            Ok(v) => match &v.req {
                None => Err(format_err!("missing transcode request message: {:?}", v)),
                Some(_) => Ok(v.req.unwrap()),
            },
            Err(v) => Err(v),
        });
    let mut tx = tx.with(|resp| futures::future::ok::<_, Error>(Into::<Message>::into(resp)));

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
                tx.send(TranscodeRespMessage::Error(format!(
                    "protocol error: {}",
                    e
                )))
                .await?;
                continue;
            }
        };
    }
    Ok(())
}

async fn handle_message<'a, T>(
    state: StreamState,
    tx: &mut T,
    eq: &mut JobEnqueuer,
    addr: &SocketAddr,
    msg: TranscodeReqMessage,
) -> Result<StreamState>
where
    T: futures::Sink<TranscodeRespMessage, Error = Error>
        + futures::SinkExt<TranscodeRespMessage>
        + std::marker::Unpin,
{
    match msg {
        TranscodeReqMessage::Handshake(_) => match &state {
            StreamState::New => {
                tx.send(TranscodeRespMessage::handshake_accepted("READY"))
                    .await?;
                return Ok(StreamState::Handshake);
            }
            other => {
                debug!("{} -> unexpected handshake from state {:?}", addr, other);
                tx.send(TranscodeRespMessage::handshake_rejected(
                    "cannot handshake at this time",
                ))
                .await?;
            }
        },
        TranscodeReqMessage::Transcode(TranscodeOpts { url }) => match &state {
            StreamState::Handshake => {
                let url = match Url::parse(&url) {
                    Ok(url) => url,
                    Err(e) => {
                        tx.send(TranscodeRespMessage::Error(format!("error: {}", e)))
                            .await?;
                        return Ok(state);
                    }
                };
                debug!("{} -> enqueuing URL {}", addr, url);

                let job = eq.enqueue_url(url).await?;
                debug!("{} -> job created, url enqueued: {:?}", addr, job);

                tx.send(TranscodeRespMessage::queued()).await?;
                return Ok(StreamState::Queued(job));
            }
            other => {
                debug!(
                    "{} -> tried to provide url at state {:?}: {:?}",
                    addr, state, other,
                );
                tx.send(TranscodeRespMessage::Error(
                    "cannot process another url at this time".into(),
                ))
                .await?;
            }
        },
        TranscodeReqMessage::Status(_) => {
            let state = match &state {
                StreamState::New => JobState::Unknown(()),
                StreamState::Handshake => JobState::NoJobs(()),
                StreamState::Queued(job) | StreamState::Done(job) => job.state.lock().await.clone(),
            };
            trace!("{} -> job status {}", addr, state);

            tx.send(TranscodeRespMessage::JobStatus(JobStatus {
                state: Some(state),
            }))
            .await?;
        }
        v => {
            debug!("{} -> Invalid message \"{:?}\"", addr, v);
            tx.send(TranscodeRespMessage::Error(format!(
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
    match ytdl::youtube_dl_download(url, Option::<&[&str]>::None).await {
        Ok(mut file) => {
            info!(
                "downloaded {} bytes",
                file.file_mut()
                    .metadata()
                    .await
                    .map(|v| v.len().to_string())
                    .unwrap_or_else(|_| format!(
                        "[couldn't query bytes for {} in {}]",
                        url,
                        file.filename()
                    ))
            );
            job.set_state_if_eq(
                JobState::Uploading(Default::default()),
                Some(JobState::Processing(Default::default())),
            )
            .await;
        }
        Err(e) => {
            warn!(
                "error processing job {:?} for {}:\n{}\n{}",
                job,
                url,
                e,
                e.chain()
                    .into_iter()
                    .map(|e| format!("{}", e))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            job.set_state_if_eq(
                JobState::Completed(RawProtoResultWrapper::from(Err::<String, _>(e))),
                Some(JobState::Processing(Default::default())),
            )
            .await;
        }
    }
}
