// ---------------------------------------------------------------------------
// x0x gossip topic names
// ---------------------------------------------------------------------------
pub const TOPIC_QUERY: &str = "av.query.v1";
pub const TOPIC_RESPONSE: &str = "av.response.v1";
pub const TOPIC_CLAIM: &str = "av.claim.v1";
pub const TOPIC_FEEDBACK: &str = "av.feedback.v1";
pub const TOPIC_NAME_QUERY: &str = "av.name.query.v1";
pub const TOPIC_NAME_RESPONSE: &str = "av.name.response.v1";
pub const TOPIC_NAME_CLAIM: &str = "av.name.claim.v1";
pub const TOPIC_PRESENCE: &str = "av.presence.v1";

// ---------------------------------------------------------------------------
// Schema / model constants
// ---------------------------------------------------------------------------
pub const SCHEMA_VERSION: u16 = 1;
pub const MINILM_DIM: u16 = 384;
pub const MINILM_MODEL_ID: &str = "all-MiniLM-L6-v2";

// ---------------------------------------------------------------------------
// URI schemes
// ---------------------------------------------------------------------------
pub const ANT_SCHEME: &str = "ant";
/// Alias accepted on input; normalised to `ant` at ingestion time.
pub const AUTONOMI_SCHEME: &str = "autonomi";

// ---------------------------------------------------------------------------
// Ranking weights (semantic search — must sum to 1.0)
// ---------------------------------------------------------------------------
pub const WEIGHT_SEMANTIC: f32 = 0.65;
pub const WEIGHT_AGREEMENT: f32 = 0.15;
pub const WEIGHT_FEEDBACK: f32 = 0.10;
pub const WEIGHT_TRUST: f32 = 0.10;

// ---------------------------------------------------------------------------
// Naming ranking weights (must sum to 1.0)
// ---------------------------------------------------------------------------
pub const NAME_WEIGHT_TRUST: f32 = 0.50;
pub const NAME_WEIGHT_AGREEMENT: f32 = 0.30;
pub const NAME_WEIGHT_RECENCY: f32 = 0.10;
pub const NAME_WEIGHT_TTL: f32 = 0.10;

// ---------------------------------------------------------------------------
// Anti-abuse defaults
// ---------------------------------------------------------------------------
/// Maximum accepted wire payload (1 MiB).
pub const MAX_PAYLOAD_BYTES: usize = 1_048_576;
pub const DEFAULT_RATE_LIMIT_PER_MINUTE: u32 = 60;
