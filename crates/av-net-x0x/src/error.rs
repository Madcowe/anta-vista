use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetError {
    #[error("http error: {0}")]
    Http(String),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("envelope too large: {size} bytes (limit {limit})")]
    TooLarge { size: usize, limit: usize },
    #[error("unsupported schema version: {0}")]
    UnsupportedVersion(u16),
    #[error("duplicate message id: {0}")]
    Duplicate(String),
    #[error("invalid payload: {0}")]
    InvalidPayload(String),
    #[error("daemon not reachable: {0}")]
    DaemonUnreachable(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub type NetResult<T> = Result<T, NetError>;
