//! The Token view's actions: fetch, start and finish a cluster-token
//! rotation (#265). Start and finish ask y/N first (`crate::confirm`).

use crate::api::{ApiClient, RotationStatus};
use crate::state::AppState;

pub(crate) async fn refresh(client: &ApiClient, state: &mut AppState) {
    apply(state, client.token_rotation().await, None);
}

pub(crate) async fn start(client: &ApiClient, state: &mut AppState) {
    let result = client.start_token_rotation().await;
    // The master rewrote cluster.token; a TUI on the master picks it up
    // now, before the old one is retired.
    client.reload_token();
    apply(
        state,
        result,
        Some("Rotation started. Update CI, laptops and scripts, then finish with f."),
    );
}

pub(crate) async fn finish(client: &ApiClient, state: &mut AppState, force: bool) {
    client.reload_token();
    apply(
        state,
        client.finish_token_rotation(force).await,
        Some("Old cluster token retired."),
    );
}

pub(crate) fn apply(
    state: &mut AppState,
    result: anyhow::Result<RotationStatus>,
    done: Option<&str>,
) {
    match result {
        Ok(status) => {
            state.rotation = Some(status);
            if let Some(msg) = done {
                state.flash(msg.to_string());
            }
        }
        Err(e) => state.error = Some(format!("Token rotation: {e}")),
    }
}

/// One agent's progress, in the CLI's words.
pub(crate) fn node_state(rotated: bool, persisted: bool) -> &'static str {
    match (rotated, persisted) {
        (true, true) => "on the new token",
        (true, false) => "new token in memory only",
        _ => "waiting (offline, or too old to rotate)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A master that answers every request with `status` and `body`.
    async fn server(status: &'static str, body: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut req = [0u8; 2048];
                let _ = sock.read(&mut req).await;
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_refused_finish_shows_the_masters_reason() {
        let reason = "agent 7 is not on the new token for good; --force to finish anyway";
        let client = ApiClient::new(&server("409 Conflict", reason).await);
        let mut state = AppState::new();
        finish(&client, &mut state, false).await;
        let err = state.error.as_deref().unwrap_or("");
        assert!(err.contains(reason), "{err}");
    }

    #[tokio::test]
    async fn a_non_admin_token_is_told_so() {
        let client = ApiClient::new(&server("403 Forbidden", "").await);
        let mut state = AppState::new();
        refresh(&client, &mut state).await;
        assert!(state.error.as_deref().unwrap_or("").contains("admin token"));
    }

    #[tokio::test]
    async fn the_status_lands_in_the_view() {
        let body = r#"{"in_progress":true,"token_file":"/root/.orca/cluster.token","nodes":[{"node_id":7,"address":"10.0.0.7:6880","rotated":true,"persisted":false,"detail":"unit passes --token"}]}"#;
        let client = ApiClient::new(&server("200 OK", body).await);
        let mut state = AppState::new();
        refresh(&client, &mut state).await;
        let r = state.rotation.expect("status");
        assert!(r.in_progress);
        assert_eq!(
            node_state(r.nodes[0].rotated, r.nodes[0].persisted),
            "new token in memory only"
        );
    }
}
