//! The master's MCP endpoint, `/mcp` over Streamable HTTP (#80).
//!
//! The same read-only server as `orca mcp`, for remote clients. Each MCP
//! request carries its caller's bearer token: `auth_middleware` checks it
//! on `/mcp` like any route, and every API call the server makes for that
//! request goes through the API router in process with the same token, so
//! the role checks per route apply exactly as for a direct call.
//!
//! Served only with `[mcp] enabled = true` and at least one API token: an
//! LLM-facing endpoint must not inherit the open mode of a token-less
//! cluster.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderValue, Method, Request, header};
use orca_core::api_types::WhoAmI;
use orca_mcp::{ApiBackend, BackendError, Caller, Connector, Limited, OrcaMcp, ReadBudget};
use rmcp::RoleServer;
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tower::ServiceExt;

use crate::auth::Principal;
use crate::state::AppState;

/// Largest API response read back in process.
const MAX_BODY: usize = 16 * 1024 * 1024;

/// `/mcp`, wrapped in the auth middleware, or `None` when it is not served.
/// `api` is the authed API router the server calls into.
pub(crate) fn router(state: &Arc<AppState>, api: Router) -> Option<Router> {
    let cfg = &state.cluster_config.mcp;
    if !cfg.enabled {
        return None;
    }
    // The lists auth_middleware reads: the runtime api_tokens (the cluster
    // token among them) and [[token]].
    let has_tokens = !state
        .api_tokens
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty()
        || !state.cluster_config.token.is_empty();
    if !has_tokens {
        tracing::warn!("[mcp] is enabled but no API tokens are configured: /mcp is not served");
        return None;
    }
    let connector = Arc::new(HttpConnector {
        api,
        per_minute: cfg.reads_per_minute,
        budgets: Mutex::new(HashMap::new()),
    });
    // Sessions (the default; idle ones close after 5 minutes) keep clients
    // on older protocol versions working; each request still carries, and
    // is checked as, its token. A browser page must not drive /mcp: an
    // Origin that is present and not in [mcp] allowed_origins is refused.
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(allowed_hosts(state))
        .with_allowed_origins(cfg.allowed_origins.clone())
        .enforce_origin_validation();
    let service = StreamableHttpService::new(
        move || Ok(OrcaMcp::per_request(connector.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    tracing::info!("serving MCP at /mcp");
    Some(
        Router::new()
            .nest_service("/mcp", service)
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                crate::auth::auth_middleware,
            )),
    )
}

/// Loopback, `cluster.domain`, and `[mcp] allowed_hosts`.
fn allowed_hosts(state: &AppState) -> Vec<String> {
    let mut hosts: Vec<String> = ["localhost", "127.0.0.1", "::1"].map(String::from).to_vec();
    hosts.extend(state.cluster_config.cluster.domain.clone());
    hosts.extend(state.cluster_config.mcp.allowed_hosts.iter().cloned());
    hosts
}

/// Resolves each request's caller from what `auth_middleware` attached.
struct HttpConnector {
    api: Router,
    per_minute: u32,
    /// One read budget per token name, shared by all its sessions.
    budgets: Mutex<HashMap<String, Arc<ReadBudget>>>,
}

impl HttpConnector {
    fn budget(&self, name: &str) -> Arc<ReadBudget> {
        let mut budgets = self.budgets.lock().unwrap_or_else(|e| e.into_inner());
        budgets
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(ReadBudget::new(self.per_minute)))
            .clone()
    }
}

#[async_trait]
impl Connector for HttpConnector {
    async fn caller(&self, context: &RequestContext<RoleServer>) -> Result<Caller, String> {
        let parts = context
            .extensions
            .get::<axum::http::request::Parts>()
            .ok_or("no HTTP request behind this MCP request")?;
        let principal = parts
            .extensions
            .get::<Principal>()
            .ok_or("the request was not authenticated")?;
        let authorization = parts
            .headers
            .get(header::AUTHORIZATION)
            .cloned()
            .ok_or("the request has no Authorization header")?;
        let backend = InProcess {
            api: self.api.clone(),
            authorization,
        };
        Ok(Caller {
            backend: Arc::new(Limited::with_budget(
                Arc::new(backend),
                self.budget(&principal.name),
            )),
            me: WhoAmI {
                actions: principal.actions(),
                name: principal.name.clone(),
                role: principal.role,
                legacy: principal.legacy,
                open: principal.open,
            },
        })
    }
}

/// The API router called in process, as the MCP request's caller.
struct InProcess {
    api: Router,
    authorization: HeaderValue,
}

#[async_trait]
impl ApiBackend for InProcess {
    async fn get(&self, path_and_query: &str) -> Result<String, BackendError> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(path_and_query)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .header("x-orca-client", "mcp-http")
            // Handlers that read the peer address get loopback.
            .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
            .body(Body::empty())
            .map_err(|e| BackendError::Transport(e.to_string()))?;
        let response = self
            .api
            .clone()
            .oneshot(request)
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), MAX_BODY)
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;
        let body = String::from_utf8_lossy(&bytes).into_owned();
        if status.is_success() {
            Ok(body)
        } else {
            Err(BackendError::Status {
                status: status.as_u16(),
                body,
            })
        }
    }
}
