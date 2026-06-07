use thiserror::Error;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: u16, got: usize },
    #[error("model profile mismatch: {detail}")]
    ProfileMismatch { detail: String },
    #[error("model not loaded: {0}")]
    ModelNotLoaded(String),
    #[error("embedding failed: {0}")]
    InferenceFailed(String),
    #[error("empty input")]
    EmptyInput,
    #[error("{0}")]
    Other(String),
}

pub type EmbedResult<T> = Result<T, EmbedError>;
