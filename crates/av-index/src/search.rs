use av_core::types::{EmbeddingRecord, ResourceDescriptor};
use av_embed::cosine_similarity;
use rusqlite::Connection;

use crate::{error::IndexResult, filter::QueryFilter};

/// A single search result with its score and contributing factors.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub resource: ResourceDescriptor,
    /// Final combined weighted score
    pub score: f32,
    /// Individual components for transparency
    pub semantic_score: f32,
    pub agreement_score: f32,
    pub feedback_score: f32,
    pub trust_score: f32,
}

/// Search for the top-k resources most similar to `query_vector`.
/// Uses brute-force cosine similarity over all stored embeddings for `profile_id`.
pub fn search_top_k(
    conn: &Connection,
    query_vector: &[f32],
    profile_id: &str,
    k: usize,
    filter: &QueryFilter,
) -> IndexResult<Vec<SearchResult>> {
    let embeddings = load_all_embeddings(conn, profile_id)?;

    // First pass: compute semantic similarity for all embeddings
    let mut candidates: Vec<(String, f32)> = embeddings
        .into_iter()
        .map(|emb| {
            let sim = cosine_similarity(query_vector, &emb.vector);
            (emb.resource_id, sim)
        })
        .collect();

    // Pre-sort by semantic to limit full scoring to a reasonable candidate set
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Full scoring pass: apply filters + trust/feedback/agreement
    let mut results: Vec<SearchResult> = Vec::new();
    for (resource_id, semantic_sim) in candidates {
        let Some(resource) = av_store::repo::resources::get(conn, &resource_id)? else {
            continue;
        };
        if !filter.scheme.allows(resource.location_scheme.as_deref()) {
            continue;
        }
        if !filter.kind.allows(&resource.kind) {
            continue;
        }
        if !filter.mime.allows(&resource.mime_type) {
            continue;
        }

        // Full weighted score
        let components = av_trust::ranking::search_score(conn, &resource_id, semantic_sim, None)
            .unwrap_or_else(|_| av_trust::ranking::ScoreComponents {
                semantic: semantic_sim.clamp(0.0, 1.0),
                agreement: 0.5,
                feedback: 0.5,
                trust: 0.5,
                combined: semantic_sim.clamp(0.0, 1.0),
            });

        results.push(SearchResult {
            score: components.combined,
            semantic_score: components.semantic,
            agreement_score: components.agreement,
            feedback_score: components.feedback,
            trust_score: components.trust,
            resource,
        });
    }

    // Re-sort by combined score
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(k);
    Ok(results)
}

fn load_all_embeddings(conn: &Connection, profile_id: &str) -> IndexResult<Vec<EmbeddingRecord>> {
    use rusqlite::params;
    let mut stmt = conn.prepare(
        "SELECT resource_id, profile_id, vector_json, l2_norm, created_at
         FROM embeddings WHERE profile_id = ?1",
    )?;
    let rows = stmt.query_map(params![profile_id], |row| {
        let vector_json: String = row.get(2)?;
        let vector: Vec<f32> = serde_json::from_str(&vector_json).unwrap_or_default();
        Ok(EmbeddingRecord {
            resource_id: row.get(0)?,
            profile_id: row.get(1)?,
            vector,
            l2_norm: row.get::<_, f64>(3)? as f32,
            created_at: row.get(4)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}
