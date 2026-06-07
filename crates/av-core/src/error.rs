use thiserror::Error;

#[derive(Debug, Error)]
pub enum AvError {
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("invalid embedding dimension: expected {expected}, got {got}")]
    InvalidDimension { expected: u16, got: usize },

    #[error("embedding profile mismatch: {detail}")]
    ProfileMismatch { detail: String },

    #[error("invalid signature")]
    InvalidSignature,

    #[error("schema version not supported: {0}")]
    UnsupportedSchemaVersion(u16),

    #[error("payload too large: {size} bytes exceeds limit {limit}")]
    PayloadTooLarge { size: usize, limit: usize },

    #[error("duplicate message id: {0}")]
    DuplicateMessageId(String),

    #[error("invalid URI: {0}")]
    InvalidUri(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type AvResult<T> = Result<T, AvError>;
