use crate::error::TrustResult;
use rusqlite::Connection;

/// Compute an agreement score for a resource in [0, 1].
///
/// Agreement = (agents who claimed this resource) / (total distinct agents seen in claims).
/// Returns 0.5 (neutral) when there is no claims data at all.
pub fn agreement_score(conn: &Connection, resource_id: &str) -> TrustResult<f32> {
    // Agents who claimed this resource
    let resource_agent_count: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT by_agent_id) FROM claims WHERE subject = ?1",
        rusqlite::params![resource_id],
        |r| r.get(0),
    )?;

    if resource_agent_count == 0 {
        return Ok(0.5); // neutral cold-start
    }

    // Total distinct agents seen across all claims
    let total_agents: i64 =
        conn.query_row("SELECT COUNT(DISTINCT by_agent_id) FROM claims", [], |r| {
            r.get(0)
        })?;

    if total_agents == 0 {
        return Ok(0.5);
    }

    let score = (resource_agent_count as f32 / total_agents as f32).min(1.0);
    Ok(score)
}
