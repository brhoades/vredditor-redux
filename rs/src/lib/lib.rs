mod file;
pub(crate) mod internal;
pub mod proto;

use std::convert::TryFrom;
use std::fs::{metadata, remove_file, File};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

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
    let pathb = file::random_temp();
    let path = pathb
        .to_str()
        .with_context(|| format!("failed to convert pathbuf to str: {:?}", pathb))?;
    debug!("temp file path created: {}", path);

    let template = format!("{}.%(ext)s", path);
    let args = vec!["-q", "--get-filename", "-o", &template];
    let filename_output = youtube_dl(url.clone(), Option::<Vec<S>>::None)
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::piped())
        .args(&args)
        .spawn()
        .with_context(|| format!("failed to spawn youtube-dl command with args: {:?}", args))?
        .wait_with_output()
        .await
        .with_context(|| format!("running youtube-dl command with args {:?}", args))?;

    if !filename_output.status.success() {
        warn!(
            "failed to query file name for: {}\n{}",
            url.as_ref().to_str().unwrap(),
            std::str::from_utf8(&filename_output.stderr).with_context(|| format!(
                "failed to parse stderr for filename output youtubedl:\n{:?}",
                filename_output.stderr
            ))?
        );
    }

    // this may not actually exist, sometimes it writes to mkv :/
    let filename = std::str::from_utf8(&filename_output.stdout)?.trim();
    info!("filename: {}", filename);
    let mut filename = Path::new(filename).to_owned();

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

    // XXX: mp4 is frequently returned but on merging it writes to mkv. Why?
    if !filename.is_file() {
        let filestr = format!(
            "{}.mkv",
            filename
                .file_stem()
                .ok_or_else(|| {
                    format_err!(
                        "youtube-dl wrote to unknown file. Expected {} with mkv or templated extension. Files are leaking",
                        filename.to_str().unwrap(),
                    )
                })?
                .to_str()
                .unwrap()
        );
        filename = filename.parent().unwrap().join(Path::new(filestr.as_str()));
    }

    if !output.status.success() {
        warn!(
            "non-zero exit status ({}) from youtube-dl download:\n{}\n{}\n",
            output.status,
            std::str::from_utf8(&output.stderr)?,
            std::str::from_utf8(&output.stdout)?,
        );
        return Err(format_err!("youtube-dl errored",));
    }

    let res = match tokio::fs::read(&filename).await {
        std::result::Result::Ok(v) => {
            debug!("read {} bytes", v.len());
            Ok(v)
        }
        std::result::Result::Err(e) => {
            warn!(
                "failed to read from youtube-dl output file: \"{}\" (from {}): {}",
                filename.to_str().unwrap(),
                path,
                e
            );
            Err(format_err!("failed to read from output file"))
        }
    };
    remove_file(filename).context("failed to delete youtube-dl file")?;
    res
}
