use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value as JSONValue;
use tokio::{io::AsyncWriteExt, process::Command, stream::StreamExt, try_join};

use crate::{file::GuardedTempFile, internal::*};

/*
format selection heirarchy:
prefer <= 1080p, <50M
"(bestaudio+bestvideo/best)[height<=1080]/best[filesize<50M]"
*/

// builds a basic youtubedl command
fn youtube_dl() -> Box<Command> {
    let mut cmd = Box::new(Command::new("youtube-dl"));
    cmd.kill_on_drop(true).args(&[
        // "-f",
        // only sends back video
        // "(bestaudio+bestvideo/best)[height<=1080][filesize<100M]",
        // same?!: "bestvideo/best[height<=1080]+bestaudio/best[height<=1080]",
        "--restrict-filenames",
    ]);

    cmd
}

#[derive(Debug, Clone, Deserialize)]
struct YTInfo {
    formats: Vec<YTFormat>,

    id: String,

    // chosen video_format+audio_format. Can also be just one.
    format_id: String,
    format: String,

    // best vcodec. can be 'none'.
    vcodec: String,
    // best acodec. can be 'none'.
    acodec: String,

    #[serde(flatten)]
    rem: HashMap<String, JSONValue>,
}

#[derive(Debug, Clone, Deserialize)]
struct YTFormat {
    format: String,
    #[serde(alias = "format_id")]
    id: String,
    #[serde(alias = "format_note")]
    note: String,

    url: String,
    filesize: Option<usize>,
    ext: String,

    #[serde(flatten)]
    video: Option<YTVideoFormat>,
    #[serde(flatten)]
    audio: Option<YTAudioFormat>,

    #[serde(flatten)]
    rem: HashMap<String, JSONValue>,
}

#[derive(Debug, Clone, Deserialize)]
struct YTVideoFormat {
    vcodec: String,
    fps: f32,
    height: u32,
    width: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct YTAudioFormat {
    acodec: String,
    abr: u32,
    asr: u32,
    tbr: f32,
}

impl YTInfo {
    fn get_format<S: AsRef<str>>(&self, id: S) -> Option<&YTFormat> {
        let id = id.as_ref();
        self.formats.iter().filter(|f| f.id == id).next()
    }

    // returns (video, audio)
    pub(crate) fn chosen_format(&self) -> (Option<&YTFormat>, Option<&YTFormat>) {
        let mut fmts = self.format_id.split("+").into_iter();
        trace!("formats found: {:?}", self.format_id.split("+"));
        (
            fmts.next().and_then(|v| self.get_format(v)),
            fmts.next().and_then(|a| self.get_format(a)),
        )
    }
}

// runs youtube_dl, collects its json, parses it, and returns info.
async fn youtube_dl_info<'a, S: AsRef<str>, A: AsRef<str>>(
    url: S,
    args: Option<&'a [A]>,
) -> Result<YTInfo> {
    let url = url.as_ref();
    let args = args.map(|v| v.iter().map(|a| a.as_ref()));

    let mut cmd = youtube_dl();
    let cmd = cmd.arg("-J").arg("--write-info-json").arg(url);
    let cmd = match args {
        Some(args) => cmd.args(args),
        _ => cmd,
    }
    .stdout(std::process::Stdio::piped())
    .stdin(std::process::Stdio::piped())
    .spawn()
    .context("spawning youtube-dl command")?;
    let res = cmd
        .wait_with_output()
        .await
        .context("running youtube-dl command")?;

    ensure!(res.status.success(), "failed to run youtube-dl on {}", url);

    std::str::from_utf8(res.stdout.as_slice())
        .ah()
        .and_then(|s| serde_json::from_str(s.trim()).ah())
        .with_context(|| format!("parsing stdout from youtube-dl for {}", url))
}

pub async fn youtube_dl_download<'b, S: AsRef<str>, B: AsRef<str>>(
    url: S,
    args: Option<&'b [B]>,
) -> Result<GuardedTempFile> {
    let url = url.as_ref();
    let ctx = || format!("for downloading url {}", url);

    // get info to retrieve target formats
    let info = youtube_dl_info(url, args).await?;
    let (video_format, audio_format) = match info.chosen_format() {
        (Some(v), Some(a)) => (v, a),
        (v, a) => {
            error!("unknown video format specifier returned. only audio? only video?");
            error!(
                "format: {} for url: {} parsed: ({:?}, {:?})",
                info.format_id, url, v, a
            );
            bail!(
                "unknown video format specifier with ({}, {})",
                v.is_some(),
                a.is_some()
            );
        }
    };

    // get their URLs.
    let (mut video, mut audio) = try_join!(
        download_to_tempfile(&video_format.url),
        download_to_tempfile(&audio_format.url)
    )
    .with_context(ctx)?;
    let (videofn, audiofn) = (video.filename().to_string(), audio.filename().to_string());

    debug!(
        "video: {} with {} bytes",
        videofn,
        video
            .len()
            .await
            .with_context(ctx)
            .context("downloaded video")?
    );
    debug!(
        "audio: {} with {} bytes",
        audiofn,
        audio
            .len()
            .await
            .with_context(ctx)
            .context("downloaded audio")?
    );

    merge_audio_video(video, audio).await.with_context(ctx)
}

async fn download_to_tempfile<U: reqwest::IntoUrl>(url: U) -> Result<GuardedTempFile> {
    let url = url.into_url()?;
    let urlstr = url.as_str();
    let ctx = || format!("for a request to {}", urlstr);

    let resp = reqwest::Client::new()
        .get(url.clone())
        .send()
        .await
        .and_then(|resp| resp.error_for_status())
        .with_context(ctx)?;
    let mut file = GuardedTempFile::new().with_context(ctx)?;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await.transpose().with_context(ctx)? {
        file.file_mut()
            .write_all(chunk.as_ref())
            .await
            .with_context(ctx)?
    }

    Ok(file)
}

// Consumes (and deletes) passed files. Runs ffmpeg on both to combine them
// into a single file, then returns it.
async fn merge_audio_video(
    video: GuardedTempFile,
    audio: GuardedTempFile,
) -> Result<GuardedTempFile> {
    let vfn = video.filename();
    let afn = audio.filename();
    let mut mergefile = GuardedTempFile::new()?;

    let child = Command::new("ffmpeg")
        .kill_on_drop(true)
        .args(&[
            "-i", vfn, "-i", afn, "-c:v", "copy", "-c:a", "aac", "-f", "mp4", "-",
        ])
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn ffmpeg subprocess")?;

    // XXX: does streaming to a file wait for the child to finish outputting complete? I assume so.
    let mut stdout = child
        .stdout
        .ok_or_else(|| format_err!("failed to get stdout handle for child ffmpeg"))?;

    // timeout after 3 minutes, running ffmpeg until it terminates or we finish copying stdout to our file
    let (res, _) = tokio::join! {
        tokio::io::copy(&mut stdout, mergefile.file_mut()),
        /*
        status = child.wait() => {
            ensure!(
                !status?.success(),
                "failed to transcode:\n{}",
                std::str::from_utf8(child.output()?.stdout.as_slice().unwrap_or("[failed to interpret stderr for ffmpeg]")
            );

        },
        */
        tokio::time::sleep(Duration::from_secs(60 * 3)),
    };

    // warn!("ffmpeg child timeout");
    // Err(format_err!("ffmpeg child timed out"))
    // child.kill().await?;

    Ok(res.map(|_| mergefile)?)
}
