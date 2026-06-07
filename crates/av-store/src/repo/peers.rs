use rusqlite::{params, Connection, Result as SqlResult};

pub struct PeerEntry {
    pub peer_id: String,
    pub metadata: serde_json::Value,
    pub last_seen_at: i64,
}

pub fn upsert(
    conn: &Connection,
    peer_id: &str,
    metadata: serde_json::Value,
    last_seen_at: i64,
) -> SqlResult<()> {
    let metadata_json = metadata.to_string();
    conn.execute(
        "INSERT OR REPLACE INTO peer_cache (peer_id, metadata_json, last_seen_at)
         VALUES (?1, ?2, ?3)",
        params![peer_id, metadata_json, last_seen_at],
    )?;
    Ok(())
}

pub fn list_recent(conn: &Connection, limit: i64) -> SqlResult<Vec<PeerEntry>> {
    let mut stmt = conn.prepare(
        "SELECT peer_id, metadata_json, last_seen_at
         FROM peer_cache ORDER BY last_seen_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        let metadata_str: String = row.get(1)?;
        Ok(PeerEntry {
            peer_id: row.get(0)?,
            metadata: serde_json::from_str(&metadata_str).unwrap_or(serde_json::Value::Null),
            last_seen_at: row.get(2)?,
        })
    })?;
    rows.collect()
}
