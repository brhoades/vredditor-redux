use std::convert::TryFrom;
use std::fmt::{Display, Formatter, Result as FmtResult};

pub use prost::{Message as ProstMessage, Oneof as OneOf};
use tokio_tungstenite::tungstenite::protocol::Message;
use tungstenite::protocol::frame::CloseFrame;

use crate::internal::*;

mod proto_inner {
    include!(concat!(env!("OUT_DIR"), "/main.rs"));
}

pub use proto_inner::{
    result::ResultInner as RawProtoResult,
    token::Token,
    transcode_req::{self, Handshake, Req as TranscodeReqMessage, Transcode as TranscodeOpts},
    transcode_resp::{
        self, job_status::State as JobState, JobStatus, Resp as TranscodeRespMessage,
    },
    Result as RawProtoResultWrapper, TranscodeReq, TranscodeResp,
};
pub use tungstenite::protocol::frame::coding::CloseCode;

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
        JobState::Queued(())
    }
}

impl Into<String> for &JobState {
    fn into(self) -> String {
        use JobState::*;

        match self {
            Queued(()) => "QUEUED".to_string(),
            NoJobs(()) => "NO JOBS".to_string(),
            Processing(_) => "PROCESSING".to_string(),
            Uploading(_) => "UPLOADING".to_string(),
            Completed(result) => match result.into() {
                Ok(u) => "COMPLETE: OK: ".to_owned() + u.as_str(),
                Err(e) => format!("COMPLETE: FAILED: {}", e),
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
 * TranscodeResp
 *************************/
#[cfg(test)]
impl TranscodeResp {
    pub fn into_state(self) -> Option<JobStatus> {
        match self.resp {
            Some(TranscodeRespMessage::JobStatus(state)) => Some(state),
            _ => None,
        }
    }
}

/************************
 * TranscodeReq
 *************************/

impl TranscodeReq {
    pub fn transcode<A: Into<String>>(msg: A) -> Self {
        Self {
            req: Some(TranscodeReqMessage::Transcode(TranscodeOpts {
                url: msg.into(),
            })),
        }
    }
}

impl TryFrom<TranscodeReq> for Message {
    type Error = anyhow::Error;

    fn try_from(lhs: TranscodeReq) -> Result<Message> {
        let mut buf = vec![];

        lhs.encode(&mut buf)?;
        Ok(Message::Binary(buf))
    }
}

/*
#[cfg(test)]
impl TranscodeReqMessage {
    // method cheats and actually parses a reqmessage
    pub fn from_req(msg: websocket_lite::Message) -> Result<Self> {
        TranscodeReq::try_from(msg)?.req
            .ok_or_else(|| format_err!("req message was missing from websocket message transcode req"))
    }
}

#[cfg(test)]
impl TranscodeReq {
    pub fn try_from(msg: websocket_lite::Message) -> Result<Self> {
        Ok(Self::decode(msg.into_data().as_ref())?)
    }
}
*/

/*
impl TryInto<Message> for TranscodeResp {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<Message> {
        let buf = vec![];

        self.encode(&mut buf)?;
        Ok(Message::Binary(buf))
    }
}
*/

// TryInto but also swallow the error into a proto.
impl Into<Message> for TranscodeResp {
    fn into(self) -> Message {
        let mut buf = vec![];

        match self.encode(&mut buf) {
            Ok(_) => Message::Binary(buf),
            Err(e) => TranscodeRespMessage::Error(format!(
                "failed to serialize message {:?}: {}",
                self, e
            ))
            .into(),
        }
    }
}

impl Into<Message> for TranscodeRespMessage {
    fn into(self) -> Message {
        TranscodeResp { resp: Some(self) }.into()
    }
}

/************************************
 * Handshake
 ***********************************/
impl Handshake {
    pub fn from_raw(_raw: Vec<u8>) -> Self {
        Self {
            // token: Some(Token::V1(raw)),
            token: None,
        }
    }

    pub fn token(&self) -> Option<&Token> {
        unimplemented!()
    }
}

/************************************
 * All client messages combined
 ***********************************/
pub enum ClientMessage {
    Transcode(TranscodeReqMessage),
    Ping(Vec<u8>),
    Close,
    CloseReason {
        code: CloseCode,
        reason: std::borrow::Cow<'static, str>,
    },
    Unsupported(Error),
}

/************************************
 * All server messages combined
 ***********************************/
pub enum ServerMessage {
    Transcode(TranscodeRespMessage),
    Pong(Vec<u8>),
    Close,
    CloseReason {
        code: CloseCode,
        reason: std::borrow::Cow<'static, str>,
    },
}

impl ServerMessage {
    pub fn handshake_accepted<T: Into<String>>(msg: T) -> Self {
        Self::Transcode(TranscodeRespMessage::Accepted(Ok(msg.into()).into()))
    }

    pub fn handshake_rejected<T: Display>(msg: T) -> Self {
        Self::Transcode(TranscodeRespMessage::Accepted(
            Into::<RawProtoResultWrapper>::into(Err::<String, _>(format_err!("{}", msg))).into(),
        ))
    }

    pub fn queued() -> Self {
        Self::Transcode(TranscodeRespMessage::JobStatus(JobStatus {
            state: Some(JobState::Queued(())),
        }))
    }

    pub fn processing() -> Self {
        Self::Transcode(TranscodeRespMessage::JobStatus(JobStatus {
            state: Some(JobState::Processing(())),
        }))
    }

    pub fn uploading() -> Self {
        Self::Transcode(TranscodeRespMessage::JobStatus(JobStatus {
            state: Some(JobState::Uploading(())),
        }))
    }

    pub fn cancelled() -> Self {
        Self::Transcode(TranscodeRespMessage::JobStatus(JobStatus {
            state: Some(JobState::Cancelled(())),
        }))
    }

    pub fn no_jobs() -> Self {
        Self::Transcode(TranscodeRespMessage::JobStatus(JobStatus {
            state: Some(JobState::NoJobs(())),
        }))
    }

    pub fn job_status(status: JobState) -> Self {
        Self::Transcode(TranscodeRespMessage::JobStatus(JobStatus {
            state: Some(status),
        }))
    }

    pub fn unknown() -> Self {
        Self::Transcode(TranscodeRespMessage::JobStatus(JobStatus {
            state: Some(JobState::Unknown(())),
        }))
    }

    pub fn error<M: Into<String>>(error: M) -> Self {
        Self::Transcode(TranscodeRespMessage::Error(error.into()))
    }

    pub fn pong(msg: Vec<u8>) -> Self {
        Self::Pong(msg)
    }

    /// close closes with no message
    pub fn close() -> Self {
        Self::Close
    }

    pub fn close_with_reason(code: CloseCode, reason: String) -> Self {
        Self::CloseReason {
            code,
            reason: reason.into(),
        }
    }
}

/************************
 * Websocket integrations
 *************************/

impl TryFrom<Message> for ClientMessage {
    type Error = anyhow::Error;

    fn try_from(msg: Message) -> Result<Self> {
        use Message::*;

        match msg {
            Binary(bytes) => TranscodeReq::decode(bytes.as_slice())
                .ah()
                .and_then(|req| {
                    req.req
                        .ok_or_else(|| format_err!("transcode req has no message"))
                })
                .map(Self::Transcode),
            Ping(payload) => Ok(Self::Ping(payload)),
            Close(None) => Ok(Self::Close),
            Close(Some(CloseFrame { code, reason })) => Ok(Self::CloseReason { code, reason }),
            other => Err(format_err!(
                "unsupported transcode request type from client: {:?}",
                other
            )),
        }
    }
}

impl TryFrom<Message> for ServerMessage {
    type Error = anyhow::Error;

    fn try_from(msg: Message) -> Result<Self> {
        use Message::*;

        match msg {
            Binary(bytes) => TranscodeResp::decode(bytes.as_slice())
                .ah()
                .and_then(|resp| {
                    resp.resp
                        .ok_or_else(|| format_err!("transcode resp has no message"))
                })
                .map(Self::Transcode),
            Pong(payload) => Ok(Self::Pong(payload)),
            Close(None) => Ok(Self::Close),
            Close(Some(CloseFrame { code, reason })) => Ok(Self::CloseReason { code, reason }),
            other => Err(format_err!(
                "unsupported transcode request type from client: {:?}",
                other
            )),
        }
    }
}

impl Into<Message> for ServerMessage {
    fn into(self) -> Message {
        match self {
            ServerMessage::Pong(payload) => Message::Pong(payload),
            ServerMessage::CloseReason { code, reason } => {
                Message::Close(Some(CloseFrame { code, reason }))
            }
            ServerMessage::Close => Message::Close(None),
            ServerMessage::Transcode(msg) => msg.into(),
        }
    }
}
