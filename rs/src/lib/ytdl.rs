use std::{collections::HashMap, ffi::OsStr, fs::remove_file, path::Path, process::Stdio};

use reqwest::IntoUrl;
use serde::Deserialize;
use serde_json::Value as JSONValue;
use tokio::{fs, io::AsyncWriteExt, process::Command, stream::StreamExt};

use crate::{file::GuardedTempFile, internal::*};

/*
format selection heirarchy:
prefer <= 1080p, <50M
"(bestaudio+bestvideo/best)[height<=1080]/best[filesize<50M]"
*/

// builds a youtubedl command which outputs json with preset format specifiers
fn youtube_dl(url: &str) -> Box<Command> {
    let mut cmd = Box::new(Command::new("youtube-dl"));
    cmd.args(&[
        "-f",
        "(bestaudio+bestvideo/best)[height<=1080]/best[filesize<50M]",
        "--restrict-filenames",
        "--maxsize",
        "100M",
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
    // #[serde(flatten)]
    // rem: HashMap<String, JSONValue>,
}

#[derive(Debug, Clone, Deserialize)]
struct YTFormat {
    format: String,
    id: String,
    note: String,

    url: String,
    filesize: usize,
    ext: String,

    #[serde(flatten)]
    video: Option<YTVideoFormat>,
    #[serde(flatten)]
    audio: Option<YTAudioFormat>,
    // #[serde(flatten)]
    // rem: HashMap<String, JSONValue>,
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

    let mut cmd = youtube_dl(url);
    let cmd = cmd.arg("-j").arg("--write-info-json");
    let cmd = match args {
        Some(args) => cmd.args(args),
        _ => cmd,
    }
    .spawn()
    .context("spawning youtube-dl command")?;
    let res = cmd
        .wait_with_output()
        .await
        .context("running youtube-dl command")?;

    ensure!(!res.status.success(), "failed to run youtube-dl on {}", url);

    std::str::from_utf8(res.stdout.as_slice())
        .ah()
        .and_then(|s| serde_json::from_str(s).ah())
        .with_context(|| format!("parsing stdout from youtube-dl for {}", url))
}

pub async fn youtube_dl_download<'b, S: AsRef<str>, B: AsRef<str>>(
    url: S,
    args: Option<&'b [B]>,
) -> Result<GuardedTempFile> {
    let url = url.as_ref();

    // get info to retrieve target formats
    let mut info = youtube_dl_info(url, args).await?;
    let mut target_fmts = &mut info.format_id.split("+");
    let (audio_format, video_format) = match (target_fmts.next(), target_fmts.next()) {
        (Some(afmt), Some(vfmt)) => (afmt, vfmt),
        other => {
            error!("unknown video format specifier returned. only audio? only video?");
            error!("format: {} for url: {}", info.format_id, url);
            bail!("unknown video format specifier: {:?}", other);
        }
    };

    // get target formats
    let (video_format, audio_format) = match info.chosen_format() {
        (Some(v), Some(a)) => (v, a),
        (v, a) => {
            error!("unknown video format specifier returned. only audio? only video?");
            error!(
                "format: {} for url: {} parsed: ({:?}, {:?})",
                info.format_id, url, v, a
            );
            bail!("unknown video format specifier: {:?}", (v, a));
        }
    };

    // get their URLs.

    let video = crate::file::GuardedTempFile::new()?;
    debug!("video path: {}", video.filename());

    download_to_tempfile(&video_format.url).await
}

async fn download_to_tempfile<U: reqwest::IntoUrl>(url: U) -> Result<GuardedTempFile> {
    let url = url.into_url()?;
    let urlstr = url.as_str();
    let resp = reqwest::Client::new()
        .get(url.clone())
        .send()
        .await
        .and_then(|resp| resp.error_for_status())
        .with_context(|| format!("making request to {}", urlstr))?;
    let mut file = GuardedTempFile::new().with_context(|| format!("for request to {}", urlstr))?;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream
        .next()
        .await
        .transpose()
        .context("while downloading to a tempfile")?
    {
        file.file_mut()
            .write_all(chunk.as_ref())
            .await
            .context("failed to write stream chunk to temp file")?
    }

    Ok(file)
}
