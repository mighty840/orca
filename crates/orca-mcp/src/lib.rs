//! Orca as a Model Context Protocol server (#80).
//!
//! Read-only: the cluster's services, logs, nodes, alerts and backups as
//! MCP resources and tools, plus prompt templates for diagnosis. Every call
//! is an `/api/v1` request made with the caller's token, so the master's
//! role checks apply unchanged. (Its audit log and rate limit cover
//! mutations and exec only, so these reads are neither audited nor
//! limited there; `orca mcp` limits its own reads, see [`limits`].) See
//! `design/mcp-endpoint.md`.

mod args;
pub mod backend;
pub mod caller;
pub mod catalog;
pub mod limits;
pub mod output;
pub mod prompts;
pub mod server;

use std::sync::Arc;

use orca_core::api_types::WhoAmI;
use rmcp::ServiceExt;

pub use backend::{ApiBackend, BackendError, HttpBackend};
pub use caller::{Caller, Connector};
pub use limits::{Limited, READS_PER_MINUTE, ReadBudget};
pub use server::OrcaMcp;

/// Ask the API who `backend`'s token belongs to.
pub async fn whoami(backend: &dyn ApiBackend) -> anyhow::Result<WhoAmI> {
    let body = backend.get("/api/v1/whoami").await?;
    Ok(serde_json::from_str(&body)?)
}

/// Serve MCP on stdin/stdout until the client disconnects. Nothing else may
/// write to stdout meanwhile: log to stderr.
pub async fn serve_stdio(backend: Arc<dyn ApiBackend>, me: WhoAmI) -> anyhow::Result<()> {
    let service = OrcaMcp::new(backend, me)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

/// Every read operation as `(tool, method, route template, action)`, for
/// the master's test that the catalog matches its `ROUTE_POLICY`.
pub fn routes() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    catalog::OPS
        .iter()
        .map(|o| (o.tool, "GET", o.route, o.action))
        .collect()
}
