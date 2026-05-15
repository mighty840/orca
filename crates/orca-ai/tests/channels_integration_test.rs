//! Integration test: Slack + webhook channels actually POST to a live HTTP
//! endpoint. Spawns a hyper server that captures the bodies it receives,
//! constructs the channels pointing at it, fires an event, and asserts the
//! captured bodies look right.
//!
//! Email is not covered here because the SMTP path needs an SMTP server in
//! the test environment; the email rendering is covered by unit tests.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use orca_ai::channels::{AlertEvent, Channel, SlackChannel, WebhookChannel};
use orca_core::types::{AlertConversation, AlertMessage, AlertSender, AlertSeverity, AlertState};

type CapturedBodies = Arc<Mutex<Vec<(String, String)>>>;

/// Spawn a hyper server that captures every request's (path, body). Returns
/// the bound address and the shared capture list.
async fn spawn_capture_server() -> (SocketAddr, CapturedBodies) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let captured: CapturedBodies = Arc::new(Mutex::new(Vec::new()));
    let captured_for_loop = captured.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let captured = captured_for_loop.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let svc = service_fn(move |req: Request<Incoming>| {
                    let captured = captured.clone();
                    async move {
                        let path = req.uri().path().to_string();
                        let body_bytes = req.into_body().collect().await.unwrap().to_bytes();
                        let body = String::from_utf8_lossy(&body_bytes).to_string();
                        captured.lock().await.push((path, body));
                        Ok::<_, hyper::Error>(Response::new(http_body_util::Full::new(
                            hyper::body::Bytes::from("ok"),
                        )))
                    }
                });
                let _ = http1::Builder::new().serve_connection(io, svc).await;
            });
        }
    });
    (addr, captured)
}

fn fake_conv() -> AlertConversation {
    AlertConversation {
        id: uuid::Uuid::now_v7(),
        service: "api".into(),
        severity: AlertSeverity::Critical,
        state: AlertState::AwaitingAction,
        started_at: chrono::Utc::now(),
        resolved_at: None,
        messages: vec![AlertMessage {
            timestamp: chrono::Utc::now(),
            sender: AlertSender::Orca,
            content: "OOM in last 5 minutes".into(),
            suggested_command: Some("orca redeploy api".into()),
        }],
    }
}

#[tokio::test]
async fn slack_channel_posts_block_kit_payload() {
    let (addr, captured) = spawn_capture_server().await;
    let url = format!("http://{addr}/slack/incoming");
    let channel = SlackChannel::new(url);

    channel
        .deliver(&fake_conv(), AlertEvent::Opened)
        .await
        .unwrap();

    // Give the server a moment to flush the capture.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let cap = captured.lock().await;
    assert_eq!(cap.len(), 1, "expected exactly one POST to slack url");
    let (path, body) = &cap[0];
    assert_eq!(path, "/slack/incoming");
    let parsed: serde_json::Value = serde_json::from_str(body).expect("slack body is JSON");
    let blocks = parsed["blocks"].as_array().expect("has blocks");
    assert!(blocks.iter().any(|b| b["type"] == "header"));
    let header_text = blocks.iter().find(|b| b["type"] == "header").unwrap()["text"]["text"]
        .as_str()
        .unwrap();
    assert!(header_text.contains("Opened"));
    assert!(header_text.contains("api"));
}

#[tokio::test]
async fn webhook_channel_posts_event_plus_conversation_json() {
    let (addr, captured) = spawn_capture_server().await;
    let url = format!("http://{addr}/hook");
    let channel = WebhookChannel::new(url);

    channel
        .deliver(&fake_conv(), AlertEvent::Remediated)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    let cap = captured.lock().await;
    assert_eq!(cap.len(), 1);
    let (path, body) = &cap[0];
    assert_eq!(path, "/hook");
    let parsed: serde_json::Value = serde_json::from_str(body).expect("webhook body is JSON");
    assert_eq!(parsed["event"], "Remediated");
    assert_eq!(parsed["conversation"]["service"], "api");
    assert_eq!(parsed["conversation"]["severity"], "critical");
}

#[tokio::test]
async fn slack_channel_surfaces_non_2xx_as_error() {
    // Spawn a server that always returns 500.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let svc = service_fn(|_req: Request<Incoming>| async {
                    let mut r =
                        Response::new(http_body_util::Full::new(hyper::body::Bytes::from("nope")));
                    *r.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                    Ok::<_, hyper::Error>(r)
                });
                let _ = http1::Builder::new().serve_connection(io, svc).await;
            });
        }
    });

    let url = format!("http://{addr}/");
    let channel = SlackChannel::new(url);
    let err = channel
        .deliver(&fake_conv(), AlertEvent::Opened)
        .await
        .expect_err("non-2xx must surface as Err");
    let msg = format!("{err}");
    assert!(
        msg.contains("500"),
        "error msg should mention status, got: {msg}"
    );
}
