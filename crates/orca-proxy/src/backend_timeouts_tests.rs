//! #187, on tokio's paused clock: minutes of upload and think time run
//! instantly and deterministically.

use std::time::Duration;

use futures_util::StreamExt;

use super::*;

fn ok_response() -> reqwest::Response {
    reqwest::Response::from(hyper::Response::new(Vec::<u8>::new()))
}

/// The prod cliff: an upload that keeps making progress for longer than
/// 120 s was cut off with a 502. It must go through.
#[tokio::test(start_paused = true)]
async fn a_long_upload_that_keeps_making_progress_is_not_cut() {
    let upload = Upload::started();
    let progress = upload.clone();
    tokio::spawn(async move {
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_secs(10)).await;
            progress.touch();
        }
        progress.finish();
    });
    // 300 s of upload, then a 60 s answer.
    let request = async {
        tokio::time::sleep(Duration::from_secs(360)).await;
        Ok(ok_response())
    };

    let result = send(request, &upload, "registry", "/v2/blobs").await;

    assert!(result.is_ok(), "{:?}", result.err());
}

#[tokio::test(start_paused = true)]
async fn a_stalled_upload_fails_after_the_upload_idle_limit() {
    let upload = Upload::started();
    let started = Instant::now();

    let result = send(std::future::pending(), &upload, "b", "/").await;

    assert!(matches!(result, Err(SendError::Idle(ref why)) if why.contains("upload stalled")));
    let waited = started.elapsed();
    assert!(waited > UPLOAD_IDLE && waited < UPLOAD_IDLE + Duration::from_secs(3));
}

/// A slow, non-streaming LLM completion: 5 minutes of think time is fine,
/// a backend that never answers is given up after RESPONSE_WAIT.
#[tokio::test(start_paused = true)]
async fn the_backend_gets_response_wait_to_answer_after_the_upload() {
    let slow = async {
        tokio::time::sleep(Duration::from_secs(300)).await;
        Ok(ok_response())
    };
    assert!(send(slow, &Upload::none(), "llm", "/v1").await.is_ok());

    let started = Instant::now();
    let never = send(std::future::pending(), &Upload::none(), "llm", "/v1").await;
    assert!(matches!(never, Err(SendError::Idle(_))));
    assert!(started.elapsed() > RESPONSE_WAIT);
}

#[tokio::test(start_paused = true)]
async fn a_response_body_that_stops_flowing_ends_with_a_timeout() {
    let chunks =
        futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"first"))])
            .chain(futures_util::stream::pending());
    let mut body = Box::pin(response_body(chunks));

    assert_eq!(
        body.next().await.unwrap().unwrap(),
        Bytes::from_static(b"first")
    );
    let err = body.next().await.unwrap().unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(body.next().await.is_none());
}

/// #192: a timeout is a 504, and says so.
#[test]
fn a_stall_is_a_gateway_timeout() {
    let e = SendError::Idle("upload stalled: no data for 120s".into());
    assert_eq!(e.status(), hyper::StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(e.client_message(), "upstream timed out");
}

/// #192: a refused connection (container being recreated) is a 502, and
/// the logged cause reaches past reqwest's bare "error sending request".
#[tokio::test]
async fn a_refused_connection_is_a_bad_gateway_with_its_cause() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let err = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap_err();
    let chain = error_chain(&err);
    let e = SendError::Backend(err);

    assert_eq!(e.status(), hyper::StatusCode::BAD_GATEWAY);
    assert_eq!(e.client_message(), "upstream unavailable");
    assert!(
        chain.to_lowercase().contains("refused"),
        "the cause must be in the log line: {chain}"
    );
}
