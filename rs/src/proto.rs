use std::convert::{TryFrom, TryInto};
use std::fmt::{Display, Formatter, Result as FmtResult};

use anyhow::{format_err, Result};
pub use prost::Message as ProstMessage;
use tokio_tungstenite::tungstenite::protocol::Message;

mod proto_inner {
    include!(concat!(env!("OUT_DIR"), "/main.rs"));
}

pub use proto_inner::{
    result::ResultInner as RawProtoResult,
    transcode_req::{self, Req as TranscodeMessage, Transcode as TranscodeOpts},
    transcode_resp::{self, job_status::State as JobState, JobStatus},
    Empty, Result as RawProtoResultWrapper, TranscodeReq,
};

/**********************
 * Results
 ***********************/

impl<T> From<Result<T>> for RawProtoResultWrapper
where
    T: Display,
{
    fn from(res: Result<T>) -> Self {
        match res {
            Ok(r) => Self {
                result_inner: Some(RawProtoResult::Ok(format!("{}", r))),
            },
            Err(e) => Self {
                result_inner: Some(RawProtoResult::Err(format!("{}", e))),
            },
        }
    }
}

impl Into<Result<String>> for &RawProtoResultWrapper {
    fn into(self) -> Result<String> {
        match &self.result_inner {
            Some(RawProtoResult::Ok(u)) => Ok(u.to_string()),
            Some(RawProtoResult::Err(e)) => Err(format_err!("{}", e)),
            None => Err(format_err!("missing inner result value")),
        }
    }
}

impl Into<Result<String>> for RawProtoResultWrapper {
    fn into(self) -> Result<String> {
        (&self).into()
    }
}

/**********************
 * JobState and Status
 ***********************/

impl<T> From<Result<T>> for JobState
where
    T: Display,
{
    fn from(res: Result<T>) -> Self {
        JobState::Completed(res.into())
    }
}

impl Default for JobState {
    fn default() -> JobState {
        JobState::Queued(Empty::default())
    }
}

impl Into<String> for &JobState {
    fn into(self) -> String {
        use JobState::*;

        match self {
            Queued(_) => "QUEUED".to_string(),
            Processing(_) => "PROCESSING".to_string(),
            Uploading(_) => "UPLOADING".to_string(),
            Completed(result) => match result.into() {
                Ok(u) => "COMPLETE: ".to_owned() + u.as_str(),
                Err(e) => format!("FAILED: {}", e),
            },
            Cancelled(_) => "CANCELLED".to_string(),
            Unknown(_) => "UNKNOWN".to_string(),
        }
    }
}

impl Into<String> for &JobStatus {
    fn into(self) -> String {
        if self.state.is_none() {
            return "UNKNOWN: NONE".to_owned();
        }

        self.state.as_ref().unwrap().into()
    }
}

/*
pub enum JobState {
    Unknown,
    Processing,
    Queued,
    Uploading,
    Completed(Result<String>),
}
*/

impl Into<String> for JobStatus {
    fn into(self) -> String {
        (&self).into()
    }
}

impl Into<String> for JobState {
    fn into(self) -> String {
        (&self).into()
    }
}

impl Display for JobStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let disp: String = self.into();
        write!(f, "{}", disp)
    }
}

impl Display for JobState {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let disp: String = self.into();
        write!(f, "{}", disp)
    }
}

pub trait JobStateCheck {
    fn is_unknown(&self) -> bool;
    fn is_processing(&self) -> bool;
    fn is_queued(&self) -> bool;
    fn is_uploading(&self) -> bool;
    fn is_completed(&self) -> bool;
}

impl JobStateCheck for JobState {
    fn is_unknown(&self) -> bool {
        match self {
            JobState::Unknown(_) => true,
            _ => false,
        }
    }

    fn is_processing(&self) -> bool {
        match self {
            JobState::Processing(_) => true,
            _ => false,
        }
    }

    fn is_queued(&self) -> bool {
        match self {
            JobState::Queued(_) => true,
            _ => false,
        }
    }

    fn is_uploading(&self) -> bool {
        match self {
            JobState::Uploading(_) => true,
            _ => false,
        }
    }

    fn is_completed(&self) -> bool {
        match self {
            JobState::Completed(_) => true,
            _ => false,
        }
    }
}

impl JobStateCheck for JobStatus {
    fn is_unknown(&self) -> bool {
        self.state.as_ref().map_or(false, |s| s.is_unknown())
    }

    fn is_processing(&self) -> bool {
        self.state.as_ref().map_or(false, |s| s.is_processing())
    }

    fn is_queued(&self) -> bool {
        self.state.as_ref().map_or(false, |s| s.is_queued())
    }

    fn is_uploading(&self) -> bool {
        self.state.as_ref().map_or(false, |s| s.is_uploading())
    }

    fn is_completed(&self) -> bool {
        self.state.as_ref().map_or(false, |s| s.is_completed())
    }
}

/************************
 * ReqState
 *************************/

/************************
 * Websocket integrations
 *************************/

impl TryFrom<Message> for TranscodeReq {
    type Error = anyhow::Error;

    fn try_from(msg: Message) -> Result<Self> {
        use Message::*;

        match msg {
            Binary(bytes) => Ok(Self::decode(bytes.as_slice())?),
            other => Err(format_err!(
                "unsupported transcode request type from client: {:?}",
                other
            )),
        }
    }
}

/*
impl TryInto<Message> for TranscodeReq {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Message> {
        let buf = vec![];

        self.encode(&mut buf)?;
        Ok(Message::Binary(buf))
    }
}

*/
