use crate::{agreement::agreement_score, error::TrustResult, feedback::feedback_score};
use av_core::constants::*;
use rusqlite::Connection;

/// All scoring components for a resource.
#[derive(Debug, Clone)]
pub struct ScoreComponents {
    pub semantic: f32,
    pub agreement: f32,
    pub feedback: f32,
    pub trust: f32,
    pub relevance: f32,
    /// Final combined score
    pub combined: f32,
}

/// Compute the full ranking score for a resource.
///
/// Formula: 0.55·semantic + 0.15·agreement + 0.10·feedback + 0.10·trust + 0.10·relevance
///
/// All inputs must be normalised to [0, 1].
pub fn search_score(
    conn: &Connection,
    resource_id: &str,
    semantic_similarity: f32,  // already in [-1,1]; clamp to [0,1] first
    by_agent_id: Option<&str>, // agent who provided the resource (for trust lookup)
    query: Option<&str>,       // query text for relevance look-up
) -> TrustResult<ScoreComponents> {
    let semantic = semantic_similarity.clamp(0.0, 1.0);
    let agreement = agreement_score(conn, resource_id)?;
    let feedback = feedback_score(conn, resource_id)?;
    let trust = agent_trust_component(conn, by_agent_id);
    let relevance = relevance_component(conn, query, resource_id);

    let combined = WEIGHT_SEMANTIC * semantic
        + WEIGHT_AGREEMENT * agreement
        + WEIGHT_FEEDBACK * feedback
        + WEIGHT_TRUST * trust
        + WEIGHT_RELEVANCE * relevance;

    Ok(ScoreComponents {
        semantic,
        agreement,
        feedback,
        trust,
        relevance,
        combined,
    })
}

/// Look up relevance judgment: 1.0 if positive, 0.5 (neutral) if none.
fn relevance_component(conn: &Connection, query: Option<&str>, resource_id: &str) -> f32 {
    let q = match query {
        Some(q) if !q.is_empty() => q,
        _ => return 0.5,
    };
    let normalized = q.trim().to_lowercase();
    match av_store::repo::relevance::get_score(conn, &normalized, resource_id) {
        Ok(Some(s)) if s > 0.0 => s.clamp(0.0, 1.0),
        _ => 0.5,
    }
}

/// Naming ranking score (no semantic component).
///
/// Formula: 0.50·trust + 0.30·agreement + 0.10·recency + 0.10·ttl_validity
pub fn name_score(trust: f32, agreement: f32, recency: f32, ttl_validity: f32) -> f32 {
    (NAME_WEIGHT_TRUST * trust
        + NAME_WEIGHT_AGREEMENT * agreement
        + NAME_WEIGHT_RECENCY * recency
        + NAME_WEIGHT_TTL * ttl_validity)
        .clamp(0.0, 1.0)
}

/// Look up an agent's trust score and normalise from [-1,1] → [0,1].
/// Returns 0.5 (neutral) if agent unknown.
fn agent_trust_component(conn: &Connection, agent_id: Option<&str>) -> f32 {
    let Some(id) = agent_id else { return 0.5 };
    match av_store::repo::trust::get(conn, id) {
        Ok(Some(state)) => ((state.trust_score + 1.0) / 2.0).clamp(0.0, 1.0),
        _ => 0.5,
    }
}
