use av_core::types::Claim;
use rusqlite::{params, Connection, Result as SqlResult};

pub fn insert(conn: &Connection, c: &Claim) -> SqlResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO claims
            (claim_id, schema_version, subject, predicate, object,
             by_agent_id, timestamp, signature)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            c.claim_id,
            c.schema_version as i64,
            c.subject,
            c.predicate,
            c.object,
            c.by_agent_id,
            c.timestamp,
            c.signature,
        ],
    )?;
    Ok(())
}

pub fn get_by_id(conn: &Connection, claim_id: &str) -> SqlResult<Option<Claim>> {
    let mut stmt = conn.prepare(
        "SELECT claim_id, schema_version, subject, predicate, object,
                by_agent_id, timestamp, signature
         FROM claims WHERE claim_id = ?1",
    )?;
    let mut rows = stmt.query(params![claim_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_claim(row)?))
    } else {
        Ok(None)
    }
}

pub fn list_by_subject(conn: &Connection, subject: &str) -> SqlResult<Vec<Claim>> {
    let mut stmt = conn.prepare(
        "SELECT claim_id, schema_version, subject, predicate, object,
                by_agent_id, timestamp, signature
         FROM claims WHERE subject = ?1 ORDER BY timestamp DESC",
    )?;
    let rows = stmt.query_map(params![subject], |row| row_to_claim(row))?;
    rows.collect()
}

pub fn list_by_agent(conn: &Connection, agent_id: &str) -> SqlResult<Vec<Claim>> {
    let mut stmt = conn.prepare(
        "SELECT claim_id, schema_version, subject, predicate, object,
                by_agent_id, timestamp, signature
         FROM claims WHERE by_agent_id = ?1 ORDER BY timestamp DESC",
    )?;
    let rows = stmt.query_map(params![agent_id], |row| row_to_claim(row))?;
    rows.collect()
}

pub fn delete(conn: &Connection, claim_id: &str) -> SqlResult<()> {
    conn.execute("DELETE FROM claims WHERE claim_id = ?1", params![claim_id])?;
    Ok(())
}

fn row_to_claim(row: &rusqlite::Row<'_>) -> SqlResult<Claim> {
    Ok(Claim {
        claim_id: row.get(0)?,
        schema_version: row.get::<_, i64>(1)? as u16,
        subject: row.get(2)?,
        predicate: row.get(3)?,
        object: row.get(4)?,
        by_agent_id: row.get(5)?,
        timestamp: row.get(6)?,
        signature: row.get(7)?,
    })
}
