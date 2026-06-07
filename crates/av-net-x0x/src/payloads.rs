use av_core::types::{Claim, FeedbackEvent, NameRecord};
use serde::{Deserialize, Serialize};

// ── Search ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPayload {
    pub query_id: String,
    pub query_text: String,
    pub max_results: u32,
    pub timeout_ms: u64,
    /// Allowed URI schemes (empty = any)
    pub allowed_schemes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceResult {
    pub resource_id: String,
    pub description_text: String,
    pub location: String,
    pub location_scheme: Option<String>,
    pub mime_type: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsePayload {
    pub query_id: String,
    pub results: Vec<ResourceResult>,
}

// ── Naming ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameQueryPayload {
    pub query_id: String,
    /// Original (human-typed) name
    pub name: String,
    /// Canonical normalized form
    pub normalized_name: String,
    /// None = any type
    pub record_type: Option<String>,
    pub max_results: u32,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameResponsePayload {
    pub query_id: String,
    pub normalized_name: String,
    pub results: Vec<NameRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameClaimPayload {
    pub record: NameRecord,
}

// ── Claims & feedback ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimPayload {
    pub claim: Claim,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackPayload {
    pub event: FeedbackEvent,
}
