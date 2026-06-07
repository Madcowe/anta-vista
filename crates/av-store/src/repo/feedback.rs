use av_core::types::{FeedbackEvent, FeedbackKind};
use rusqlite::{params, Connection, Result as SqlResult};

pub fn insert(conn: &Connection, f: &FeedbackEvent) -> SqlResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO feedback_events
            (feedback_id, schema_version, query_text, resource_id, by_agent_id,
             kind, timestamp, signature)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            f.feedback_id,
            f.schema_version as i64,
            f.query_text,
            f.resource_id,
            f.by_agent_id,
            kind_to_str(&f.kind),
            f.timestamp,
            f.signature,
        ],
    )?;
    Ok(())
}

pub fn list_by_resource(conn: &Connection, resource_id: &str) -> SqlResult<Vec<FeedbackEvent>> {
    let mut stmt = conn.prepare(
        "SELECT feedback_id, schema_version, query_text, resource_id, by_agent_id,
                kind, timestamp, signature
         FROM feedback_events WHERE resource_id = ?1 ORDER BY timestamp DESC",
    )?;
    let rows = stmt.query_map(params![resource_id], |row| row_to_feedback(row))?;
    rows.collect()
}

pub fn list_by_agent(conn: &Connection, agent_id: &str) -> SqlResult<Vec<FeedbackEvent>> {
    let mut stmt = conn.prepare(
        "SELECT feedback_id, schema_version, query_text, resource_id, by_agent_id,
                kind, timestamp, signature
         FROM feedback_events WHERE by_agent_id = ?1 ORDER BY timestamp DESC",
    )?;
    let rows = stmt.query_map(params![agent_id], |row| row_to_feedback(row))?;
    rows.collect()
}

fn row_to_feedback(row: &rusqlite::Row<'_>) -> SqlResult<FeedbackEvent> {
    let kind_str: String = row.get(5)?;
    Ok(FeedbackEvent {
        feedback_id: row.get(0)?,
        schema_version: row.get::<_, i64>(1)? as u16,
        query_text: row.get(2)?,
        resource_id: row.get(3)?,
        by_agent_id: row.get(4)?,
        kind: str_to_kind(&kind_str),
        timestamp: row.get(6)?,
        signature: row.get(7)?,
    })
}

fn kind_to_str(k: &FeedbackKind) -> &str {
    match k {
        FeedbackKind::Useful => "useful",
        FeedbackKind::NotUseful => "not_useful",
        FeedbackKind::Incorrect => "incorrect",
        FeedbackKind::HighConfidence => "high_confidence",
    }
}

fn str_to_kind(s: &str) -> FeedbackKind {
    match s {
        "useful" => FeedbackKind::Useful,
        "not_useful" => FeedbackKind::NotUseful,
        "incorrect" => FeedbackKind::Incorrect,
        "high_confidence" => FeedbackKind::HighConfidence,
        _ => FeedbackKind::Useful,
    }
}
