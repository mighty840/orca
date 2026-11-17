//! How the MCP server reaches orca: only through `/api/v1`, so every call
//! goes through the master's auth, role checks and audit log.

use async_trait::async_trait;

/// A failed API call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// The API answered with an error status.
    #[error("the orca API answered {status}: {body}")]
    Status { status: u16, body: String },
    /// The API could not be reached.
    #[error("cannot reach the orca API: {0}")]
    Transport(String),
}

/// Read access to the orca API. PR 2 only reads.
#[async_trait]
pub trait ApiBackend: Send + Sync {
    /// `GET path_and_query` (e.g. `/api/v1/status?project=web`); the body.
    async fn get(&self, path_and_query: &str) -> Result<String, BackendError>;
}

/// How long one API call may take. Log tails from a remote node go through
/// the agent, so this is generous.
pub const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The orca API over HTTP with a bearer token: what `orca mcp` uses.
pub struct HttpBackend {
    base_url: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl HttpBackend {
    pub fn new(base_url: &str, token: Option<String>) -> Self {
        // A hung master must not hang the client's tool call forever.
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            client,
        }
    }
}

#[async_trait]
impl ApiBackend for HttpBackend {
    async fn get(&self, path_and_query: &str) -> Result<String, BackendError> {
        let mut req = self
            .client
            .get(format!("{}{path_and_query}", self.base_url))
            // The audit log records which client made a call.
            .header("x-orca-client", "mcp-stdio");
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| BackendError::Transport(e.to_string()))?;
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
