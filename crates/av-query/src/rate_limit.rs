use std::collections::HashMap;
use std::time::Instant;

/// Configuration for the rate limiter.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum tokens (burst capacity)
    pub capacity: f32,
    /// Tokens added per second (sustained rate)
    pub refill_rate: f32,
}

impl RateLimitConfig {
    pub fn new(capacity: f32, refill_rate: f32) -> Self {
        Self {
            capacity,
            refill_rate,
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        // Default: 60 messages/min burst of 20
        Self {
            capacity: 20.0,
            refill_rate: 1.0,
        }
    }
}

#[derive(Debug)]
struct Bucket {
    tokens: f32,
    last_refill: Instant,
}

impl Bucket {
    fn new(capacity: f32) -> Self {
        Self {
            tokens: capacity,
            last_refill: Instant::now(),
        }
    }

    fn refill(&mut self, rate: f32, capacity: f32) {
        let elapsed = self.last_refill.elapsed().as_secs_f32();
        self.tokens = (self.tokens + elapsed * rate).min(capacity);
        self.last_refill = Instant::now();
    }

    fn try_consume(&mut self, rate: f32, capacity: f32) -> bool {
        self.refill(rate, capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Per-agent token-bucket rate limiter.
pub struct RateLimiter {
    buckets: HashMap<String, Bucket>,
    config: RateLimitConfig,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            buckets: HashMap::new(),
            config,
        }
    }

    /// Check if an agent is allowed to send a message.
    /// Returns `true` if allowed (token consumed), `false` if rate-limited.
    pub fn check_and_consume(&mut self, agent_id: &str) -> bool {
        let config = &self.config;
        let bucket = self
            .buckets
            .entry(agent_id.to_string())
            .or_insert_with(|| Bucket::new(config.capacity));
        bucket.try_consume(config.refill_rate, config.capacity)
    }

    /// Remove stale buckets for agents not seen recently (optional housekeeping).
    pub fn evict_inactive(&mut self) {
        self.buckets
            .retain(|_, b| b.last_refill.elapsed().as_secs() < 3600);
    }
}
