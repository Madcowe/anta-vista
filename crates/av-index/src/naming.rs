use av_core::types::{NameRecord, normalize_name};
use rusqlite::{Connection, params};

use crate::{error::IndexResult, filter::SchemeFilter};

#[derive(Debug, Clone)]
pub struct NameResult {
    pub record: NameRecord,
    /// Combined ranking score [0, 1]
    pub score: f32,
    pub trust_score: f32,
    pub agreement_score: f32,
    pub recency_score: f32,
    pub ttl_valid: bool,
}

/// Look up a name exactly (case-insensitive via normalized_name).
/// Returns all matching records sorted by ranking score descending.
/// Optionally filtered by scheme.
pub fn lookup_name(
    conn: &Connection,
    name: &str,
    scheme_filter: &SchemeFilter,
    now_secs: i64,
) -> IndexResult<Vec<NameResult>> {
    let normalized = normalize_name(name);
    let records = av_store::repo::names::get_by_normalized_name(conn, &normalized)?;

    if records.is_empty() {
        return Ok(vec![]);
    }

    // Filter by scheme
    let records: Vec<NameRecord> = records
        .into_iter()
        .map(|mut r| {
            if let Some(ref s) = r.target_scheme {
                r.target_scheme = Some(av_core::types::normalize_scheme(s));
            }
            if let Some(ref tc) = r.target_canonical {
                if tc.starts_with("autonomi://") {
                    r.target_canonical = Some(tc.replace("autonomi://", "ant://"));
                }
            } else {
                if r.target.starts_with("autonomi://") {
                    r.target_canonical = Some(r.target.replace("autonomi://", "ant://"));
                } else if r.target.starts_with("ant://") {
                    r.target_canonical = Some(r.target.clone());
                }
            }
            r
        })
        .filter(|r| scheme_filter.allows(r.target_scheme.as_deref()))
        .collect();

    if records.is_empty() {
        return Ok(vec![]);
    }

    // Find min/max timestamps for recency normalization
    let (min_ts, max_ts) = records.iter().fold((i64::MAX, i64::MIN), |(mn, mx), r| {
        (mn.min(r.timestamp), mx.max(r.timestamp))
    });
    let ts_range = (max_ts - min_ts).max(1) as f32;

    // Total distinct agents who have published any name record (for agreement denominator)
    let total_agents: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT by_agent_id) FROM name_records",
            [],
            |row| row.get(0),
        )
        .unwrap_or(1)
        .max(1);

    let mut results: Vec<NameResult> = records
        .into_iter()
        .map(|r| {
            let ttl_valid = now_secs < r.timestamp + r.ttl_secs as i64;
            let recency_score = (r.timestamp - min_ts) as f32 / ts_range;

            // Trust: look up the publishing agent's score and normalise [-1,1] → [0,1]
            let trust_score = match av_store::repo::trust::get(conn, &r.by_agent_id) {
                Ok(Some(state)) => ((state.trust_score + 1.0) / 2.0).clamp(0.0, 1.0),
                _ => 0.5,
            };

            // Agreement: fraction of distinct agents who published a record for this name
            let name_agent_count: i64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT by_agent_id) FROM name_records WHERE normalized_name = ?1",
                    params![&r.normalized_name],
                    |row| row.get(0),
                )
                .unwrap_or(1);
            let agreement_score = (name_agent_count as f32 / total_agents as f32).min(1.0);

            let ttl_validity = if ttl_valid { 1.0_f32 } else { 0.0_f32 };
            let score =
                av_trust::ranking::name_score(trust_score, agreement_score, recency_score, ttl_validity);

            NameResult {
                record: r,
                score,
                trust_score,
                agreement_score,
                recency_score,
                ttl_valid,
            }
        })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(results)
}
