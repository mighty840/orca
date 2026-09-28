use super::*;

#[test]
fn appending_keeps_only_the_newest_lines() {
    let mut buf = String::from("1\n2\n3\n");
    assert_eq!(append_capped(&mut buf, "4\n5\n", 4), 2);
    assert_eq!(buf, "2\n3\n4\n5\n");
}

#[test]
fn a_partial_line_is_completed_by_the_next_chunk() {
    let mut buf = String::new();
    assert_eq!(append_capped(&mut buf, "GET /a 2", 10), 0);
    assert_eq!(append_capped(&mut buf, "00\nGET /b 200\n", 10), 2);
    assert_eq!(buf, "GET /a 200\nGET /b 200\n");
}

fn logs_view(service: &str) -> AppState {
    let mut state = AppState::new();
    state.view = View::Logs {
        service: service.into(),
    };
    state
}

#[test]
fn the_first_chunk_replaces_the_polled_tail_then_chunks_append() {
    let mut state = logs_view("api");
    state.logs = "old polled tail\n".into();
    state.bg.follow.fresh = true;
    apply_chunk(&mut state, "api", "a\n");
    apply_chunk(&mut state, "api", "b\n");
    assert_eq!(state.logs, "a\nb\n");
}

#[test]
fn a_scrolled_up_view_holds_its_place() {
    let mut state = logs_view("api");
    state.service_scroll = 5;
    apply_chunk(&mut state, "api", "x\ny\n");
    assert_eq!(state.service_scroll, 7);
}

#[test]
fn chunks_for_another_service_are_ignored() {
    let mut state = logs_view("api");
    state.logs = "api\n".into();
    apply_chunk(&mut state, "db", "db\n");
    assert_eq!(state.logs, "api\n");
}

#[test]
fn an_ended_stream_switches_that_service_to_polling() {
    // An agent's service: the master sends one batch and closes.
    let mut state = logs_view("api");
    assert!(is_followed(&state, "api"));
    apply_ended(&mut state, "api", None);
    assert!(!is_followed(&state, "api"));
    assert!(is_followed(&state, "db"));
}

#[test]
fn a_polled_tail_does_not_overwrite_a_followed_log() {
    let mut state = logs_view("api");
    state.logs = "streamed\n".into();
    crate::background::apply(
        &mut state,
        Fetched::Logs {
            service: "api".into(),
            result: Ok("polled\n".into()),
        },
    );
    assert_eq!(state.logs, "streamed\n");
}

/// A server that streams two chunks of log output, then closes.
async fn streaming_server() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut req = [0u8; 1024];
        let _ = sock.read(&mut req).await;
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        sock.write_all(head.as_bytes()).await.unwrap();
        sock.write_all(b"2\r\na\n\r\n").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        sock.write_all(b"2\r\nb\n\r\n0\r\n\r\n").await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_stream_appends_as_it_arrives_then_falls_back_to_polling() {
    let client = ApiClient::new(&streaming_server().await);
    let mut state = logs_view("api");
    sync(&client, &mut state);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while is_followed(&state, "api") && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        crate::background::drain(&mut state);
    }
    assert_eq!(state.logs, "a\nb\n");
    assert!(
        !is_followed(&state, "api"),
        "the ended stream is polled now"
    );
}
