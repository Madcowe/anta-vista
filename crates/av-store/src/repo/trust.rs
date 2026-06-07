use av_core::types::TrustState;
use rusqlite::{params, Connection, Result as SqlResult};

pub fn upsert(conn: &Connection, t: &TrustState) -> SqlResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO trust_state
            (subject_agent_id, trust_score, evidence_count, last_updated_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            t.subject_agent_id,
            t.trust_score as f64,
            t.evidence_count as i64,
            t.last_updated_at,
        ],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, agent_id: &str) -> SqlResult<Option<TrustState>> {
    let mut stmt = conn.prepare(
        "SELECT subject_agent_id, trust_score, evidence_count, last_updated_at
         FROM trust_state WHERE subject_agent_id = ?1",
    )?;
    let mut rows = stmt.query(params![agent_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_trust(row)?))
    } else {
        Ok(None)
    }
}

pub fn list_all(conn: &Connection) -> SqlResult<Vec<TrustState>> {
    let mut stmt = conn.prepare(
        "SELECT subject_agent_id, trust_score, evidence_count, last_updated_at
         FROM trust_state ORDER BY trust_score DESC",
    )?;
    let rows = stmt.query_map([], |row| row_to_trust(row))?;
    rows.collect()
}

fn row_to_trust(row: &rusqlite::Row<'_>) -> SqlResult<TrustState> {
    Ok(TrustState {
        subject_agent_id: row.get(0)?,
        trust_score: row.get::<_, f64>(1)? as f32,
        evidence_count: row.get::<_, i64>(2)? as u32,
        last_updated_at: row.get(3)?,
    })
}
