//! Who a request comes from, and how to reach the API as them.
//!
//! `orca mcp` (stdio) serves one caller: the token it started with. The
//! master's `/mcp` serves many over HTTP; each request carries its own
//! token, which the master's auth middleware has already checked.

use std::sync::Arc;

use async_trait::async_trait;
use orca_core::api_types::WhoAmI;
use rmcp::RoleServer;
use rmcp::service::RequestContext;

use crate::backend::ApiBackend;

/// The caller of one MCP request and an API backend acting as them.
pub struct Caller {
    pub backend: Arc<dyn ApiBackend>,
    pub me: WhoAmI,
}

/// Resolves the caller from a request, for servers with many callers.
#[async_trait]
pub trait Connector: Send + Sync {
    async fn caller(&self, context: &RequestContext<RoleServer>) -> Result<Caller, String>;
}

/// Where an [`crate::OrcaMcp`] gets its caller.
#[derive(Clone)]
pub(crate) enum Source {
    /// One caller for the server's lifetime (stdio).
    Fixed(Arc<dyn ApiBackend>, WhoAmI),
    /// Resolved per request (the master's `/mcp`).
    PerRequest(Arc<dyn Connector>),
}

impl Source {
    pub(crate) async fn caller(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<Caller, String> {
        match self {
            Self::Fixed(backend, me) => Ok(Caller {
                backend: backend.clone(),
                me: me.clone(),
            }),
            Self::PerRequest(connector) => connector.caller(context).await,
        }
    }
}
