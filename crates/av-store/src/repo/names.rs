use av_core::types::{NameRecord, NameRecordType};
use rusqlite::{params, Connection, Result as SqlResult};

pub fn insert(conn: &Connection, nr: &NameRecord) -> SqlResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO name_records
            (record_id, schema_version, normalized_name, original_name, record_type,
             target, target_scheme, target_canonical, ttl_secs, by_agent_id, timestamp, signature)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            nr.record_id,
            nr.schema_version as i64,
            nr.normalized_name,
            nr.original_name,
            record_type_to_str(&nr.record_type),
            nr.target,
            nr.target_scheme,
            nr.target_canonical,
            nr.ttl_secs as i64,
            nr.by_agent_id,
            nr.timestamp,
            nr.signature,
        ],
    )?;
    Ok(())
}

pub fn get_by_normalized_name(conn: &Connection, name: &str) -> SqlResult<Vec<NameRecord>> {
    let mut stmt = conn.prepare(
        "SELECT record_id, schema_version, normalized_name, original_name, record_type,
                target, target_scheme, target_canonical, ttl_secs, by_agent_id, timestamp, signature
         FROM name_records WHERE normalized_name = ?1 ORDER BY timestamp DESC",
    )?;
    let rows = stmt.query_map(params![name], |row| row_to_name_record(row))?;
    rows.collect()
}

pub fn list_by_scheme(conn: &Connection, scheme: &str) -> SqlResult<Vec<NameRecord>> {
    let mut stmt = conn.prepare(
        "SELECT record_id, schema_version, normalized_name, original_name, record_type,
                target, target_scheme, target_canonical, ttl_secs, by_agent_id, timestamp, signature
         FROM name_records WHERE target_scheme = ?1 ORDER BY timestamp DESC",
    )?;
    let rows = stmt.query_map(params![scheme], |row| row_to_name_record(row))?;
    rows.collect()
}

pub fn delete(conn: &Connection, record_id: &str) -> SqlResult<()> {
    conn.execute(
        "DELETE FROM name_records WHERE record_id = ?1",
        params![record_id],
    )?;
    Ok(())
}

fn row_to_name_record(row: &rusqlite::Row<'_>) -> SqlResult<NameRecord> {
    let record_type_str: String = row.get(4)?;
    Ok(NameRecord {
        record_id: row.get(0)?,
        schema_version: row.get::<_, i64>(1)? as u16,
        normalized_name: row.get(2)?,
        original_name: row.get(3)?,
        record_type: str_to_record_type(&record_type_str),
        target: row.get(5)?,
        target_scheme: row.get(6)?,
        target_canonical: row.get(7)?,
        ttl_secs: row.get::<_, i64>(8)? as u32,
        by_agent_id: row.get(9)?,
        timestamp: row.get(10)?,
        signature: row.get(11)?,
    })
}

fn record_type_to_str(t: &NameRecordType) -> &str {
    match t {
        NameRecordType::A => "a",
        NameRecordType::Txt => "txt",
        NameRecordType::Uri => "uri",
        NameRecordType::Service => "service",
    }
}

fn str_to_record_type(s: &str) -> NameRecordType {
    match s {
        "a" => NameRecordType::A,
        "txt" => NameRecordType::Txt,
        "uri" => NameRecordType::Uri,
        "service" => NameRecordType::Service,
        _ => NameRecordType::Txt,
    }
}
