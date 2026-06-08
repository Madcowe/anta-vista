//! Attack scenario builders for adversarial testing

use uuid::Uuid;

/// Sybil cluster: N low-trust agents with corroborating claims
pub struct SybilCluster {
    pub agent_ids: Vec<String>,
    pub trust_level: f64,
}

impl SybilCluster {
    pub fn new(size: usize, trust_level: f64) -> Self {
        let agent_ids = (0..size)
            .map(|i| format!("{:064x}", i))
            .collect();
        Self { agent_ids, trust_level }
    }

    pub fn add_claim(&self, claim: &str) -> Vec<(String, String)> {
        self.agent_ids
            .iter()
            .map(|id| (id.clone(), claim.to_string()))
            .collect()
    }
}

/// Replay attack: duplicate message_id or stale sent_at
#[derive(Clone, Debug)]
pub struct ReplayEnvelope {
    pub message_id: String,
    pub sent_at: i64,
    pub is_stale: bool,
}

impl ReplayEnvelope {
    pub fn new_duplicate(message_id: &str) -> Self {
        Self {
            message_id: message_id.to_string(),
            sent_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            is_stale: false,
        }
    }

    pub fn new_stale(message_id: &str, minutes_ago: i64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Self {
            message_id: message_id.to_string(),
            sent_at: now - (minutes_ago * 60),
            is_stale: true,
        }
    }
}

/// Poisoned name record: conflicting claims from untrusted agents
#[derive(Clone, Debug)]
pub struct PoisonedNameRecord {
    pub name: String,
    pub conflicting_claims: Vec<String>,
    pub empty_signature: bool,
}

impl PoisonedNameRecord {
    pub fn new(name: &str, empty_signature: bool) -> Self {
        Self {
            name: name.to_string(),
            conflicting_claims: vec![
                format!("{}:alternative1", name),
                format!("{}:alternative2", name),
            ],
            empty_signature,
        }
    }
}

/// Resource poisoner: misleading descriptions designed to game similarity scoring
#[derive(Clone, Debug)]
pub struct ResourcePoisoner {
    pub description: String,
    pub fake_tokens: Vec<String>,
}

impl ResourcePoisoner {
    pub fn new_with_misleading_description(target_resource: &str) -> Self {
        let description = format!(
            "{}. This is actually about {}. Very important {}. Definitely {}.",
            target_resource, target_resource, target_resource, target_resource
        );
        let fake_tokens = vec![
            target_resource.to_string(),
            "important".to_string(),
            "definitely".to_string(),
            "very".to_string(),
        ];
        Self { description, fake_tokens }
    }
}

/// Size bypass attack: SSE wrapper under limit but decoded payload exceeds
#[derive(Clone, Debug)]
pub struct SizeBypassPayload {
    pub wrapper_size: usize,
    pub decoded_size: usize,
}

impl SizeBypassPayload {
    pub fn create_near_limit(max_bytes: usize) -> Self {
        Self {
            wrapper_size: max_bytes - 100,
            decoded_size: max_bytes + 1000,
        }
    }
}

/// Helper to generate unique test identifiers
pub fn unique_id() -> String {
    Uuid::new_v4().to_string()
}
