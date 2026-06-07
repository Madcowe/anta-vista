use thiserror::Error;

#[derive(Debug, Error)]
pub enum QueryError {
    #[error("rate limit exceeded for agent {0}")]
    RateLimited(String),
    #[error("payload too large: {size} bytes (limit {limit})")]
    TooLarge { size: usize, limit: usize },
    #[error("agent {0} is blocked (strike threshold exceeded)")]
    AgentBlocked(String),
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("trust error: {0}")]
    Trust(String),
    #[error("{0}")]
    Other(String),
}

pub type QueryResult<T> = Result<T, QueryError>;
