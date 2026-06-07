use thiserror::Error;

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("embed error: {0}")]
    Embed(String),
    #[error("no embeddings found for profile {0}")]
    NoEmbeddings(String),
    #[error("invalid filter: {0}")]
    InvalidFilter(String),
    #[error("{0}")]
    Other(String),
}

pub type IndexResult<T> = Result<T, IndexError>;
