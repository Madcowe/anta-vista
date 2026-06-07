use rusqlite::{params, Connection, Result as SqlResult};

pub struct QueryCacheEntry {
    pub query_id: String,
    pub query_text: String,
    pub result: serde_json::Value,
    pub created_at: i64,
    pub expires_at: i64,
}

pub fn insert(conn: &Connection, entry: &QueryCacheEntry) -> SqlResult<()> {
    let result_json = entry.result.to_string();
    conn.execute(
        "INSERT OR REPLACE INTO query_cache
            (query_id, query_text, result_json, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            entry.query_id,
            entry.query_text,
            result_json,
            entry.created_at,
            entry.expires_at,
        ],
    )?;
    Ok(())
}

/// Returns the entry only if it has not expired (expires_at > now in Unix seconds).
pub fn get_if_valid(
    conn: &Connection,
    query_id: &str,
    now: i64,
) -> SqlResult<Option<QueryCacheEntry>> {
    let mut stmt = conn.prepare(
        "SELECT query_id, query_text, result_json, created_at, expires_at
         FROM query_cache WHERE query_id = ?1 AND expires_at > ?2",
    )?;
    let mut rows = stmt.query(params![query_id, now])?;
    if let Some(row) = rows.next()? {
        let result_str: String = row.get(2)?;
        Ok(Some(QueryCacheEntry {
            query_id: row.get(0)?,
            query_text: row.get(1)?,
            result: serde_json::from_str(&result_str).unwrap_or(serde_json::Value::Null),
            created_at: row.get(3)?,
            expires_at: row.get(4)?,
        }))
    } else {
        Ok(None)
    }
}

/// Delete all entries whose expires_at <= now.
pub fn purge_expired(conn: &Connection, now: i64) -> SqlResult<usize> {
    let count = conn.execute(
        "DELETE FROM query_cache WHERE expires_at <= ?1",
        params![now],
    )?;
    Ok(count)
}
