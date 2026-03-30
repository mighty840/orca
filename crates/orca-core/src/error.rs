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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_error_display() {
        let e = OrcaError::Config("missing field".into());
        assert_eq!(e.to_string(), "config error: missing field");
    }

    #[test]
    fn workload_not_found_display() {
        let e = OrcaError::WorkloadNotFound { name: "web".into() };
        assert_eq!(e.to_string(), "workload 'web' not found");
    }

    #[test]
    fn runtime_error_display() {
        let e = OrcaError::Runtime("container crashed".into());
        assert_eq!(e.to_string(), "runtime error: container crashed");
    }
}
