use rusqlite::{params, Connection, Result as SqlResult};

/// Upsert a relevance judgment for a (query, resource_id) pair.
pub fn upsert(conn: &Connection, query: &str, resource_id: &str, score: f32) -> SqlResult<()> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        "INSERT OR REPLACE INTO relevance_judgments
            (normalized_query, resource_id, score, created_at, updated_at)
         VALUES (?1, ?2, ?3,
            COALESCE((SELECT created_at FROM relevance_judgments
                      WHERE normalized_query = ?1 AND resource_id = ?2), ?4),
            ?4)",
        params![query, resource_id, score, now],
    )?;
    Ok(())
}

/// Get the relevance score for a (query, resource_id) pair.
/// Returns `None` when no judgment exists (neutral).
pub fn get_score(conn: &Connection, query: &str, resource_id: &str) -> SqlResult<Option<f32>> {
    let mut stmt = conn.prepare(
        "SELECT score FROM relevance_judgments
         WHERE normalized_query = ?1 AND resource_id = ?2",
    )?;
    let mut rows = stmt.query(params![query, resource_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}
