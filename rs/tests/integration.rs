use log::debug;
use std::convert::TryFrom;
use std::time::Duration;

use anyhow::*;
use futures::{Sink, SinkExt, Stream, StreamExt, TryStreamExt};
use tokio::{
    select,
    time::{interval, sleep},
};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use lib::{proto::*, run_server};

macro_rules! assert_status {
    ($res:expr, $status:pat) => {
        assert!(matches!($res, TranscodeRespMessage::JobStatus(_)));

        let status = match $res {
            TranscodeRespMessage::JobStatus(status) => status.state,
            _ => unreachable!(),
        };

        assert!(status.is_some());
        let status = status.unwrap();
        assert!(matches!(status, $status));
    };
}

#[tokio::test]
async fn test_client() -> Result<()> {
    let addr = "localhost:50023";

    // we cancel by returning an error from any ne of these
    select! {
        res = run_server(addr.to_owned()) => {
            return res;
        },
        res = run_test_client(addr.to_owned()) => {
            return res;
        },
    }
}

#[cfg(test)]
async fn run_test_client<T: AsRef<str>>(addr: T) -> Result<()> {
    let mut tries: i8 = 0;
    let url = url::Url::parse(&format!("ws://{}", addr.as_ref()))?;

    // just keeping this in a loop so I don't have to type the return value of ClientBuilder.
    while tries < 5 {
        // wait for the server to come up.
        sleep(Duration::from_millis(10)).await;

        let stream = connect_async(&url)
            .await
            .with_context(|| format!("failed to connect to ws server @ {}", url));
        let (ws_stream, _) = match (stream, tries) {
            (Ok(s), _) => s,
            (Err(_), 0..=3) => {
                tries += 1;
                continue;
            }
            (Err(e), tries) => {
                return Err(e).with_context(|| {
                    format!("failed to connect to ws server after {} tries", tries)
                });
            }
        };
        let mut stream = ws_stream.map_err(|e| format_err!("{}", e));

        assert!(matches!(
            send_and_get(&mut stream, TranscodeReqMessage::Handshake(())).await?,
            TranscodeRespMessage::Accepted(_),
        ));

        let res = send_and_get(
            &mut stream,
            TranscodeReqMessage::Transcode(TranscodeOpts {
                url: "https://mobile.twitter.com/KatieDaviscourt/status/1317765993385529346"
                    .to_string(),
            }),
        )
        .await?;
        assert_status!(res, JobState::Queued(_));

        let mut int = interval(Duration::from_millis(100));
        let mut times: i8 = 0;
        while let _ = int.next().await {
            assert!(times < 20, "exceeded maximum number of tries");
            times += 1;

            let res = send_and_get(&mut stream, TranscodeReqMessage::Status(())).await?;
            assert!(matches!(res, TranscodeRespMessage::JobStatus(_)));
            let state = match res {
                TranscodeRespMessage::JobStatus(s) => s.state,
                _ => unreachable!(),
            }
            .expect("state should not be null");

            debug!("status: \"{}\"", state);
            match state {
                JobState::Queued(_) => (),
                JobState::Processing(_) => (),
                JobState::Uploading(_) | JobState::Completed(_) => break,
                res => panic!("unknown state: {}", res),
            }
        }

        break;
    }

    Ok(())
}

#[cfg(test)]
async fn send_and_get<T>(stream: &mut T, req: TranscodeReqMessage) -> Result<TranscodeRespMessage>
where
    T: Sink<Message> + Stream<Item = Result<Message>> + Unpin,
    <T as Sink<Message>>::Error: Send + Sync + std::error::Error + 'static,
{
    let req = Message::try_from(TranscodeReq { req: Some(req) })?;

    stream
        .send(req.clone())
        .await
        .with_context(|| format!("from request {:?}", req))?;

    stream
        .next()
        .await
        .ok_or_else(|| format_err!("no message received from server",))?
        .with_context(|| format!("from server in response to message: {:?}", req))
        .and_then(|v| {
            TranscodeResp::try_from(v)
                .with_context(|| format!("from server response to: {:?}", req))
                .map(|v| v.resp)
                .transpose()
                .unwrap()
        })
}
