//! #80 PR 3: `/mcp` on the master, over raw JSON-RPC as a remote client
//! sends it. Needs the `mcp` feature.
#![cfg(feature = "mcp")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use orca_control::state::AppState;
use orca_core::config::{ApiToken, ClusterConfig, McpConfig, Role};
use serde_json::{Value, json};
use tokio::sync::RwLock;

const VIEWER: &str = "mcp-viewer-token";
const DEPLOYER: &str = "mcp-deployer-token";

async fn start(mcp: McpConfig, with_tokens: bool) -> u16 {
    start_with(mcp, with_tokens, vec![]).await
}

async fn start_with(mcp: McpConfig, with_tokens: bool, api_tokens: Vec<String>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let token = |name: &str, value: &str, role| ApiToken {
        name: name.into(),
        value: value.into(),
        role,
    };
    let config = ClusterConfig {
        token: if with_tokens {
            vec![
                token("claude", VIEWER, Role::Viewer),
                token("ci", DEPLOYER, Role::Deployer),
            ]
        } else {
            vec![]
        },
        api_tokens,
        mcp,
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
    tokio::time::sleep(Duration::from_millis(100)).await;
    port
}

fn enabled() -> McpConfig {
    McpConfig {
        enabled: true,
        ..Default::default()
    }
}

/// One MCP client session: initialize, then requests with its session id.
struct Session {
    port: u16,
    token: Option<&'static str>,
    host: Option<&'static str>,
    origin: Option<&'static str>,
    id: Option<String>,
    next: u64,
}

impl Session {
    fn new(port: u16, token: Option<&'static str>) -> Self {
        Self {
            port,
            token,
            host: None,
            origin: None,
            id: None,
            next: 1,
        }
    }

    async fn post(&mut self, body: Value) -> reqwest::Response {
        let mut req = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{}/mcp", self.port))
            .header("accept", "application/json, text/event-stream")
            .json(&body);
        if let Some(t) = self.token {
            req = req.bearer_auth(t);
        }
        if let Some(h) = self.host {
            req = req.header("host", h);
        }
        if let Some(o) = self.origin {
            req = req.header("origin", o);
        }
        if let Some(id) = &self.id {
            req = req.header("mcp-session-id", id);
        }
        req.send().await.unwrap()
    }

    async fn initialize(&mut self) -> u16 {
        let resp = self
            .post(
                json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"}}}),
            )
            .await;
        let status = resp.status().as_u16();
        self.id = resp
            .headers()
            .get("mcp-session-id")
            .map(|v| v.to_str().unwrap().to_string());
        if status == 200 {
            self.post(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .await;
        }
        status
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let resp = self
            .post(json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params}))
            .await;
        assert_eq!(resp.status(), 200);
        let body = resp.text().await.unwrap();
        // An SSE stream: the answer is the `data:` line holding JSON.
        body.lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .find_map(|d| serde_json::from_str(d).ok())
            .unwrap_or_else(|| panic!("no JSON-RPC answer in {body:?}"))
    }

    async fn tool(&mut self, name: &str) -> (bool, String) {
        let r = self
            .call("tools/call", json!({"name": name, "arguments": {}}))
            .await;
        let result = &r["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        (result["isError"] == true, text)
    }
}

#[tokio::test]
async fn a_viewer_lists_and_calls_read_tools_as_itself() {
    let port = start(enabled(), true).await;
    let mut s = Session::new(port, Some(VIEWER));
    assert_eq!(s.initialize().await, 200);

    let tools = s.call("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"get_status"), "{tools}");
    assert!(!names.contains(&"secrets_inventory"), "{names:?}");

    let (error, text) = s.tool("whoami").await;
    assert!(!error, "{text}");
    assert!(
        text.contains("\"claude\""),
        "the inner call ran as the caller: {text}"
    );
}

#[tokio::test]
async fn a_tool_beyond_the_callers_role_is_refused_by_the_inner_call() {
    let port = start(enabled(), true).await;
    let mut s = Session::new(port, Some(DEPLOYER));
    s.initialize().await;

    let (error, text) = s.tool("secrets_inventory").await;
    assert!(error);
    assert!(text.contains("'secrets' permission"), "{text}");
}

#[tokio::test]
async fn mcp_needs_a_valid_token_and_a_known_host() {
    let port = start(enabled(), true).await;
    assert_eq!(Session::new(port, None).initialize().await, 401);
    assert_eq!(Session::new(port, Some("nope")).initialize().await, 401);

    let mut foreign = Session::new(port, Some(VIEWER));
    foreign.host = Some("evil.example.com");
    let status = foreign.initialize().await;
    assert!((400..500).contains(&status), "DNS rebinding: {status}");

    let mut page = Session::new(port, Some(VIEWER));
    page.origin = Some("https://evil.example.com");
    assert_eq!(page.initialize().await, 403, "a browser page");
}

#[tokio::test]
async fn the_cluster_token_is_refused_unless_allowed() {
    let port = start_with(enabled(), true, vec!["cluster-token".into()]).await;
    assert_eq!(
        Session::new(port, Some("cluster-token")).initialize().await,
        403
    );
    assert_eq!(Session::new(port, Some(VIEWER)).initialize().await, 200);

    // A cluster with only the cluster token: /mcp is served (and warns),
    // and the token works once allowed.
    let allowed = McpConfig {
        allow_cluster_token: true,
        ..enabled()
    };
    let port = start_with(allowed, false, vec!["cluster-token".into()]).await;
    assert_eq!(
        Session::new(port, Some("cluster-token")).initialize().await,
        200
    );
}

#[tokio::test]
async fn every_api_call_made_through_mcp_is_audited_as_the_caller() {
    let port = start_with(enabled(), true, vec!["admin-token".into()]).await;
    let mut s = Session::new(port, Some(VIEWER));
    s.initialize().await;
    s.tool("get_status").await;
    s.tool("secrets_inventory").await; // refused: still recorded

    let events: Vec<Value> = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/v1/audit"))
        .bearer_auth("admin-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let seen: Vec<(&str, &str, u64, &str)> = events
        .iter()
        .map(|e| {
            (
                e["principal"].as_str().unwrap_or("-"),
                e["route"].as_str().unwrap(),
                e["status"].as_u64().unwrap(),
                e["client"].as_str().unwrap_or("-"),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("claude", "/api/v1/secrets/usage", 403, "mcp-http"),
            ("claude", "/api/v1/status", 200, "mcp-http"),
        ]
    );
}

#[tokio::test]
async fn mcp_is_not_served_when_disabled_or_on_a_cluster_without_tokens() {
    // Not mounted: an unknown route, which a viewer token may not even
    // probe (403) and which is not there (404).
    let off = start(McpConfig::default(), true).await;
    let status = Session::new(off, Some(VIEWER)).initialize().await;
    assert!(matches!(status, 403 | 404), "{status}");

    let open = start(enabled(), false).await;
    assert_eq!(Session::new(open, None).initialize().await, 404);
}

#[tokio::test]
async fn the_read_budget_is_per_token_and_shared_by_its_sessions() {
    let port = start(
        McpConfig {
            reads_per_minute: 1,
            ..enabled()
        },
        true,
    )
    .await;
    let mut first = Session::new(port, Some(VIEWER));
    first.initialize().await;
    let mut second = Session::new(port, Some(VIEWER));
    second.initialize().await;
    let mut other = Session::new(port, Some(DEPLOYER));
    other.initialize().await;

    assert!(!first.tool("whoami").await.0);
    let (error, text) = second.tool("whoami").await;
    assert!(error && text.contains("read limit"), "{text}");
    assert!(
        !other.tool("whoami").await.0,
        "another token has its own budget"
    );
}
