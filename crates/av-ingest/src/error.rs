use thiserror::Error;

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("file too small to detect mime type")]
    FileTooSmall,
    #[error("unsupported mime type: {0}")]
    UnsupportedMime(String),
    #[error("{0}")]
    Other(String),
}

pub type IngestResult<T> = Result<T, IngestError>;
