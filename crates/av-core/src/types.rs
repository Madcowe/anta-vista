use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

// ---------------------------------------------------------------------------
// Embedding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingProfile {
    /// e.g. "all-MiniLM-L6-v2"
    pub model_id: String,
    /// Exact pinned runtime/model version
    pub model_version: String,
    /// Vector dimensionality (384 for MiniLM)
    pub dim: u16,
    /// Whether vectors are L2-normalised before storage
    pub normalized: bool,
    /// Tokenizer/pre-process contract hash or version
    pub preproc_version: String,
}

// ---------------------------------------------------------------------------
// Resource
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ResourceKind {
    Text,
    Image,
    Audio,
    File,
    Pdf,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceDescriptor {
    pub id: String,
    pub kind: ResourceKind,
    pub location: String,
    pub location_scheme: Option<String>,
    pub location_canonical: Option<String>,
    pub mime_type: String,
    pub filename: Option<String>,
    pub metadata_json: serde_json::Value,
    pub description_text: String,
    pub created_at: i64,
}

// ---------------------------------------------------------------------------
// Embedding record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingRecord {
    pub resource_id: String,
    pub profile_id: String,
    pub vector: Vec<f32>,
    pub l2_norm: f32,
    pub created_at: i64,
}

// ---------------------------------------------------------------------------
// Claim
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Claim {
    pub schema_version: u16,
    pub claim_id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub by_agent_id: String,
    pub timestamp: i64,
    pub signature: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Feedback
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FeedbackKind {
    Useful,
    NotUseful,
    Incorrect,
    HighConfidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeedbackEvent {
    pub schema_version: u16,
    pub feedback_id: String,
    pub query_text: String,
    pub resource_id: String,
    pub by_agent_id: String,
    pub kind: FeedbackKind,
    pub timestamp: i64,
    pub signature: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Trust
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustState {
    pub subject_agent_id: String,
    pub trust_score: f32,
    pub evidence_count: u32,
    pub last_updated_at: i64,
}

// ---------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NameRecordType {
    A,
    Txt,
    Uri,
    Service,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NameRecord {
    pub schema_version: u16,
    pub record_id: String,
    pub normalized_name: String,
    pub original_name: String,
    pub record_type: NameRecordType,
    pub target: String,
    pub target_scheme: Option<String>,
    pub target_canonical: Option<String>,
    pub ttl_secs: u32,
    pub by_agent_id: String,
    pub timestamp: i64,
    pub signature: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Wire protocol envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageEnvelope {
    pub schema_version: u16,
    pub message_id: String,
    pub sent_at: i64,
    pub from_agent_id: String,
    pub kind: MessageKind,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Query,
    Response,
    Claim,
    Feedback,
    NameQuery,
    NameResponse,
    NameClaim,
    Presence,
}

// ---------------------------------------------------------------------------
// Name / scheme normalisation helpers
// ---------------------------------------------------------------------------

/// Unicode NFC + case-fold (lowercase) normalisation for human-readable names.
pub fn normalize_name(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

/// Canonicalise URI schemes — maps "autonomi" → "ant", passes others through.
pub fn normalize_scheme(scheme: &str) -> String {
    let lower = scheme.to_lowercase();
    match lower.as_str() {
        "autonomi" => "ant".to_owned(),
        _ => lower,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    fn roundtrip<T>(value: &T) -> T
    where
        T: serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
        let json = serde_json::to_string(value).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn test_embedding_profile_roundtrip() {
        let ep = EmbeddingProfile {
            model_id: "all-MiniLM-L6-v2".into(),
            model_version: "1.0.0".into(),
            dim: 384,
            normalized: true,
            preproc_version: "v1-sha256-abc123".into(),
        };
        assert_eq!(ep, roundtrip(&ep));
    }

    #[test]
    fn test_resource_descriptor_roundtrip() {
        let rd = ResourceDescriptor {
            id: "res-001".into(),
            kind: ResourceKind::Text,
            location: "ant://abc123".into(),
            location_scheme: Some("ant".into()),
            location_canonical: None,
            mime_type: "text/plain".into(),
            filename: Some("readme.txt".into()),
            metadata_json: serde_json::json!({"author": "alice"}),
            description_text: "A readme file".into(),
            created_at: 1_700_000_000,
        };
        assert_eq!(rd, roundtrip(&rd));
    }

    #[test]
    fn test_claim_roundtrip() {
        let c = Claim {
            schema_version: 1,
            claim_id: "claim-001".into(),
            subject: "res-001".into(),
            predicate: "describes".into(),
            object: "example content".into(),
            by_agent_id: "agent-xyz".into(),
            timestamp: 1_700_000_001,
            signature: vec![0xde, 0xad, 0xbe, 0xef],
        };
        assert_eq!(c, roundtrip(&c));
    }

    #[test]
    fn test_feedback_event_roundtrip() {
        let fe = FeedbackEvent {
            schema_version: 1,
            feedback_id: "fb-001".into(),
            query_text: "find rust tutorials".into(),
            resource_id: "res-001".into(),
            by_agent_id: "agent-xyz".into(),
            kind: FeedbackKind::Useful,
            timestamp: 1_700_000_002,
            signature: vec![0x01, 0x02],
        };
        assert_eq!(fe, roundtrip(&fe));
    }

    #[test]
    fn test_trust_state_roundtrip() {
        let ts = TrustState {
            subject_agent_id: "agent-abc".into(),
            trust_score: 0.85,
            evidence_count: 42,
            last_updated_at: 1_700_000_003,
        };
        assert_eq!(ts, roundtrip(&ts));
    }

    #[test]
    fn test_name_record_roundtrip() {
        let nr = NameRecord {
            schema_version: 1,
            record_id: "nr-001".into(),
            normalized_name: "my-service".into(),
            original_name: "My-Service".into(),
            record_type: NameRecordType::Uri,
            target: "ant://deadbeef".into(),
            target_scheme: Some("ant".into()),
            target_canonical: None,
            ttl_secs: 3600,
            by_agent_id: "agent-xyz".into(),
            timestamp: 1_700_000_004,
            signature: vec![0xca, 0xfe],
        };
        assert_eq!(nr, roundtrip(&nr));
    }

    #[test]
    fn test_message_envelope_roundtrip() {
        let me = MessageEnvelope {
            schema_version: 1,
            message_id: "msg-001".into(),
            sent_at: 1_700_000_005,
            from_agent_id: "agent-xyz".into(),
            kind: MessageKind::Query,
            payload: serde_json::json!({"q": "hello"}),
        };
        assert_eq!(me, roundtrip(&me));
    }

    #[test]
    fn test_message_kind_snake_case_serialization() {
        let kind = MessageKind::NameQuery;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, r#""name_query""#);
    }

    #[test]
    fn test_name_normalization() {
        assert_eq!(normalize_name("Hello World"), "hello world");
        assert_eq!(normalize_name("CAFÉ"), "café");
    }

    #[test]
    fn test_scheme_normalization() {
        assert_eq!(normalize_scheme("autonomi"), "ant");
        assert_eq!(normalize_scheme("AUTONOMI"), "ant");
        assert_eq!(normalize_scheme("https"), "https");
        assert_eq!(normalize_scheme("ANT"), "ant");
    }
}
