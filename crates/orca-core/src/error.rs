use thiserror::Error;

#[derive(Error, Debug)]
pub enum OrcaError {
    #[error("config error: {0}")]
    Config(String),

    #[error("runtime error: {0}")]
    Runtime(String),

    #[error("workload '{name}' not found")]
    WorkloadNotFound { name: String },

    #[error("node '{id}' not found")]
    NodeNotFound { id: String },

    #[error("scheduler error: {0}")]
    Scheduler(String),

    #[error("consensus error: {0}")]
    Consensus(String),

    #[error("proxy error: {0}")]
    Proxy(String),

    #[error("secret '{key}' not found")]
    SecretNotFound { key: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, OrcaError>;
