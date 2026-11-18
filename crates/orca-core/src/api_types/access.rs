//! Who is calling, and what callers did: `GET /api/v1/whoami` and
//! `GET /api/v1/audit` (#80, shared with #259 and #265).

use serde::{Deserialize, Serialize};

use crate::config::Role;

/// Response from `GET /api/v1/whoami`: the caller's token and what it may do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoAmI {
    /// The `[[token]]` name, `legacy-<n>` for an `api_tokens` entry (the
    /// cluster token among them), or `anonymous` when no tokens are
    /// configured.
    pub name: String,
    pub role: Role,
    /// An `api_tokens` entry: admin, unnamed, and possibly the agents' join
    /// credential. Clients that act on their own (MCP) should refuse it.
    pub legacy: bool,
    /// No tokens are configured: every caller is admin.
    #[serde(default)]
    pub open: bool,
    /// The actions this role may perform, as `ROUTE_POLICY` names them.
    pub actions: Vec<String>,
}

/// One audited API request: every mutating request and every exec, allowed
/// or denied. Only the route template is kept, never the query or the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub at: chrono::DateTime<chrono::Utc>,
    /// Token name, as in [`WhoAmI::name`]; `None` when no valid token was
    /// presented.
    pub principal: Option<String>,
    pub method: String,
    /// Route template, e.g. `/api/v1/services/{name}/redeploy`.
    pub route: String,
    /// The `{name}` path segment when the route has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub status: u16,
    pub latency_ms: u64,
    /// `X-Orca-Client` as the caller claims it (cli, tui, mcp-stdio, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
}
