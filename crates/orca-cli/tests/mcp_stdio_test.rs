//! #80: `orca mcp` against a real API. Its stdout is the protocol channel,
//! so every line there must be a JSON-RPC message: a log line would break
//! the client. And the cluster token is refused unless asked for.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use orca_control::state::AppState;
use orca_core::config::{ApiToken, ClusterConfig, Role};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::RwLock;

const CLUSTER_TOKEN: &str = "cluster-token-for-mcp-test";
const VIEWER_TOKEN: &str = "viewer-token-for-mcp-test";

async fn api() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = ClusterConfig {
        api_tokens: vec![CLUSTER_TOKEN.into()],
        token: vec![ApiToken {
            name: "mcp".into(),
            value: VIEWER_TOKEN.into(),
            role: Role::Viewer,
        }],
        ..Default::default()
    };
    let state = Arc::new(AppState::new(
        config,
        Arc::new(orca_core::testing::MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ));
    let app = orca_control::api::router(state);
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    format!("http://127.0.0.1:{port}")
}

fn orca_mcp(api: &str, token: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_orca"));
    cmd.args(["mcp", "--api", api, "--token", token])
        .env("RUST_LOG", "debug")
        .env_remove("ORCA_LOG_FILE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

#[tokio::test]
async fn stdout_carries_only_protocol_messages() {
    let api = api().await;
    let mut child = orca_mcp(&api, VIEWER_TOKEN).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();

    for msg in [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"}}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    ] {
        stdin
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
    }

    let mut tools = None;
    while tools.is_none() {
        let line = tokio::time::timeout(Duration::from_secs(20), stdout.next_line())
            .await
            .expect("an answer within 20s")
            .unwrap()
            .expect("stdout still open");
        let msg: serde_json::Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("not JSON on stdout ({e}): {line}"));
        assert_eq!(msg["jsonrpc"], "2.0", "{line}");
        if msg["id"] == 2 {
            tools = Some(msg["result"]["tools"].clone());
        }
    }
    let names: Vec<&str> = tools
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"get_status"), "{names:?}");
    assert!(
        !names.contains(&"secrets_inventory"),
        "a viewer token: {names:?}"
    );
}

#[tokio::test]
async fn the_cluster_token_is_refused_by_default() {
    let api = api().await;
    let out = orca_mcp(&api, CLUSTER_TOKEN).output().await.unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "nothing on the protocol channel");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--allow-cluster-token"), "{stderr}");
}

#[tokio::test]
async fn an_unreachable_api_fails_on_stderr_and_leaves_stdout_empty() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    drop(listener);
    let out = orca_mcp(&api, VIEWER_TOKEN).output().await.unwrap();
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot ask"));
}
