use crate::{
    error::{QueryError, QueryResult},
    rate_limit::RateLimiter,
};
use av_core::constants::MAX_PAYLOAD_BYTES;

/// Validates an incoming message before processing.
/// Checks size cap and per-agent rate limit.
pub struct PayloadGuard {
    pub rate_limiter: RateLimiter,
    pub max_bytes: usize,
}

impl PayloadGuard {
    pub fn new(rate_limiter: RateLimiter) -> Self {
        Self {
            rate_limiter,
            max_bytes: MAX_PAYLOAD_BYTES,
        }
    }

    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// Validate an incoming message from `agent_id` of `payload_size` bytes.
    /// Returns `Ok(())` if allowed, `Err` if it should be dropped.
    pub fn check(&mut self, agent_id: &str, payload_size: usize) -> QueryResult<()> {
        if payload_size > self.max_bytes {
            return Err(QueryError::TooLarge {
                size: payload_size,
                limit: self.max_bytes,
            });
        }
        if !self.rate_limiter.check_and_consume(agent_id) {
            return Err(QueryError::RateLimited(agent_id.to_string()));
        }
        Ok(())
    }
}
