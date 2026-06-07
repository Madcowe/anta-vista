use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use av_core::constants::{MAX_PAYLOAD_BYTES, SCHEMA_VERSION};
use av_core::types::{MessageEnvelope, MessageKind};
use uuid::Uuid;

use crate::error::{NetError, NetResult};

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Build a signed envelope ready for publishing.
pub fn build_envelope(
    from_agent_id: &str,
    kind: MessageKind,
    payload: serde_json::Value,
) -> MessageEnvelope {
    MessageEnvelope {
        schema_version: SCHEMA_VERSION,
        message_id: Uuid::new_v4().to_string(),
        sent_at: now_secs(),
        from_agent_id: from_agent_id.to_string(),
        kind,
        payload,
    }
}

/// Validate an incoming envelope. Returns Err on any violation.
pub fn validate_envelope(env: &MessageEnvelope, raw_size: usize) -> NetResult<()> {
    if raw_size > MAX_PAYLOAD_BYTES {
        return Err(NetError::TooLarge {
            size: raw_size,
            limit: MAX_PAYLOAD_BYTES,
        });
    }
    if env.schema_version != SCHEMA_VERSION {
        return Err(NetError::UnsupportedVersion(env.schema_version));
    }
    if env.message_id.is_empty() {
        return Err(NetError::InvalidPayload("missing message_id".into()));
    }
    if env.from_agent_id.is_empty() {
        return Err(NetError::InvalidPayload("missing from_agent_id".into()));
    }
    Ok(())
}

/// Simple time-bounded deduplication cache for message_ids.
pub struct DedupeCache {
    seen: HashMap<String, Instant>,
    ttl: Duration,
}

impl DedupeCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            seen: HashMap::new(),
            ttl,
        }
    }

    /// Returns `true` if this message_id is a duplicate (already seen within TTL).
    pub fn is_duplicate(&mut self, message_id: &str) -> bool {
        self.evict_expired();
        if self.seen.contains_key(message_id) {
            return true;
        }
        self.seen.insert(message_id.to_string(), Instant::now());
        false
    }

    fn evict_expired(&mut self) {
        let ttl = self.ttl;
        self.seen.retain(|_, inserted| inserted.elapsed() < ttl);
    }
}
