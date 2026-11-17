//! `orca mcp`: the read-only MCP server on stdin/stdout (#80).
//!
//! Stdout carries the protocol only; `main` sends tracing to stderr for
//! this command, and nothing here prints.

use std::sync::Arc;

use anyhow::bail;
use orca_mcp::{ApiBackend, HttpBackend};

pub async fn handle_mcp(
    api: &str,
    token: Option<&str>,
    allow_cluster_token: bool,
) -> anyhow::Result<()> {
    let api = crate::handlers::ops::resolve_api(api);
    let token = crate::handlers::server::read_token(token);
    let backend: Arc<dyn ApiBackend> = Arc::new(HttpBackend::new(&api, token));
    let me = match orca_mcp::whoami(backend.as_ref()).await {
        Ok(me) => me,
        Err(e)
            if matches!(
                e.downcast_ref::<orca_mcp::BackendError>(),
                Some(orca_mcp::BackendError::Status { status: 404, .. })
            ) =>
        {
            bail!(
                "the master at {api} is too old for `orca mcp` (it has no /api/v1/whoami); upgrade it"
            )
        }
        Err(e) => bail!("cannot ask {api} who this token is: {e}"),
    };
    if me.legacy && !allow_cluster_token {
        bail!(
            "refusing to serve MCP with '{}', a legacy api_tokens entry: it is admin, and the \
             cluster token is also the agents' join credential. Create a token for it with \
             `orca token create --name mcp --role viewer` and pass --token, or pass \
             --allow-cluster-token",
            me.name
        );
    }
    if me.open {
        tracing::warn!(
            "the cluster has no API tokens: every MCP client can read everything, secrets \
             inventory included"
        );
    }
    tracing::info!(token = %me.name, role = ?me.role, "serving MCP on stdio for {api}");
    orca_mcp::serve_stdio(backend, me).await
}
