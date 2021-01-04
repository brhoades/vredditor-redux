use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use serde_json::Value as JSONValue;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    try_join,
};
use tokio_compat_02::FutureExt;

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

fn default_codec() -> String {
    "none".to_string()
}

#[derive(Debug, Clone, Deserialize)]
struct YTInfo {
    formats: Vec<YTFormat>,

    id: String,

    // chosen video_format+audio_format. Can also be just one.
    format_id: String,
    format: String,

    // best vcodec. can be 'none'.
    #[serde(default = "default_codec")]
    vcodec: String,
    // best acodec. can be 'none'.
    #[serde(default = "default_codec")]
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
    note: Option<String>,

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
    vcodec: Option<String>,
    fps: f32,
    height: u32,
    width: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct YTAudioFormat {
    acodec: Option<String>,
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
    .stdout(Stdio::piped())
    .stdin(Stdio::piped())
    .spawn()
    .context("spawning youtube-dl command")?;
    let res = cmd
        .wait_with_output()
        .await
        .context("running youtube-dl command")?;

    ensure!(res.status.success(), "failed to run youtube-dl on {}", url);

    std::str::from_utf8(res.stdout.as_slice())
        .ah()
        .and_then(|s| {
            serde_json::from_str(s.trim())
                .ah()
                .with_context(|| format!("when parsing: {}", s.trim()))
        })
        .with_context(|| format!("youtube-dl for {} produced unexpected output", url))
}

pub(crate) async fn youtube_dl_download<'b, S: AsRef<str>, B: AsRef<str>>(
    clients: &Clients,
    url: S,
    args: Option<&'b [B]>,
) -> Result<GuardedTempFile> {
    let url = url.as_ref();
    let ctx = || format!("for downloading url {}", url);

    // get info to retrieve target formats
    let info = youtube_dl_info(url, args).await?;
    let (video_format, audio_format) = match info.chosen_format() {
        (Some(v), a) => (Some(v), a),
        (v, a) => {
            error!("unknown video format specifier returned. only audio? only video?");
            error!(
                "format: {} for url: {} parsed: ({:?}, {:?})",
                info.format_id, url, v, a
            );
            bail!(
                "unsupported audio/video format combination: {}/{}",
                a.map(|a| a.format.clone()).unwrap_or("none".to_owned()),
                v.map(|v| v.format.clone()).unwrap_or("none".to_owned()),
            );
        }
    };

    // HACK: replace .m3u8 with .ts to work around HLS. We should read the file to determine that, but this'll work great.
    let video_format_url = video_format.as_ref().map(|v| v.url.replace(".m3u8", ".ts"));
    let audio_format_url = audio_format.as_ref().map(|u| u.url.clone());

    // get their URLs.
    let (mut video, mut audio) = try_join!(
        clients.maybe_download_to_tempfile(video_format_url.as_ref()),
        clients.maybe_download_to_tempfile(audio_format_url.as_ref())
    )
    .with_context(ctx)?;
    if let Some(ref mut video) = video {
        debug!(
            "video: {} with {} bytes from {}",
            video.path_string(),
            video
                .len()
                .await
                .with_context(ctx)
                .context("downloaded video")?,
            video_format_url.unwrap(),
        );
    }

    if let Some(ref mut audio) = audio {
        debug!(
            "audio: {} with {} bytes from {}",
            audio.path_string(),
            audio
                .len()
                .await
                .with_context(ctx)
                .context("downloaded audio")?,
            audio_format_url.unwrap(),
        );
    }

    merge_audio_video(video, audio).await.with_context(ctx)
}

impl Clients {
    // downloads to a temp file if passed something, otherwise succeeds with None.
    async fn maybe_download_to_tempfile<U: reqwest::IntoUrl>(
        &self,
        url: Option<U>,
    ) -> Result<Option<GuardedTempFile>> {
        if let Some(url) = url {
            self.download_to_tempfile(url.into_url()?).await.map(Some)
        } else {
            Ok(None)
        }
    }

    async fn download_to_tempfile<U: reqwest::IntoUrl>(&self, url: U) -> Result<GuardedTempFile> {
        let url = url.into_url()?;
        let urlstr = url.as_str();
        let ctx = || format!("for a request to {}", urlstr);

        let resp: reqwest::Response = self
            .http_client
            .get(url.clone())
            .send()
            .compat() // XXX: remove with reqwest on tokio 0.3+
            .await
            .and_then(|resp| resp.error_for_status())
            .with_context(ctx)?;
        let mut file = GuardedTempFile::new().await.with_context(ctx)?;

        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await.transpose().with_context(ctx)? {
            file.file_mut()
                .write_all(chunk.as_ref())
                .await
                .with_context(ctx)?
        }

        Ok(file)
    }
}

// Consumes (and deletes) passed files. Runs ffmpeg on both to combine them
// into a single file, then returns it.
async fn merge_audio_video(
    video: Option<GuardedTempFile>,
    audio: Option<GuardedTempFile>,
) -> Result<GuardedTempFile> {
    let vfn = video.as_ref().map(|v| v.pathstr());
    let afn = audio.as_ref().map(|a| a.pathstr());
    let mut mergefile = GuardedTempFile::new().await?;
    let mergefn = mergefile.path_string();

    let mut args = vec![
        "-y", // to clobber. This only works since GuardedTempFile explicitly unlinks the file by name.
    ];
    if let Some(vfn) = vfn {
        args.extend(vec!["-i", vfn]);
    }
    if let Some(afn) = afn {
        args.extend(vec!["-i", afn]);
    }
    args.extend(vec!["-c", "copy", "-f", "mp4", &mergefn]);

    let mut child = Command::new("ffmpeg")
        .kill_on_drop(true)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn ffmpeg subprocess")?;

    // XXX: does streaming to a file wait for the child to finish outputting complete? I assume so.
    // timeout after 3 minutes, running ffmpeg until it terminates or we finish copying stdout to our file
    (tokio::select! {
        // tokio::io::copy(&mut stdout, mergefile.file_mut()),
        status = child.wait() => {
            let status = status.context("when running ffmpeg")?;
            let mut stderr_buff = "".to_owned();
            let stderr = if let Some(mut serr) = child.stderr.take() {
                match serr.read_to_string(&mut stderr_buff).await {
                    Ok(_) => stderr_buff,
                    Err(e) => {
                        warn!("ffmpeg didn't output utf8: {}", e);
                        "[failed to parse utf8 stderr for ffmpeg]".to_owned()
                    },
                }
            } else {
                warn!("ffmpeg exited and failed without a stderr handle: {:?}", child);
                "[failed to interpret stderr for ffmpeg]".to_owned()
            };

            ensure!(
                status.success(),
                "failed to transcode:\n{}",
                stderr,
            );
            Result::<()>::Ok(())
        },
        _ = tokio::time::sleep(Duration::from_secs(15)) => {
            child.kill().await?;
            return Err(format_err!("timed out after 15 seconds"));
        }
    })
    .context("failed to run ffmpeg")?;

    // ffmpeg clobbered our file. reopen it.
    mergefile.reopen().await?;

    // warn!("ffmpeg child timeout");
    // Err(format_err!("ffmpeg child timed out"))
    // child.kill().await?;

    Ok(mergefile)
}
