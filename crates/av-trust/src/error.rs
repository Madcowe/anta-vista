use thiserror::Error;

#[derive(Debug, Error)]
pub enum TrustError {
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("{0}")]
    Other(String),
}

pub type TrustResult<T> = Result<T, TrustError>;
