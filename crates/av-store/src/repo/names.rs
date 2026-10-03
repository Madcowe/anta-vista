use av_core::types::{NameRecord, NameRecordType};
use rusqlite::{Connection, Result as SqlResult, params};

/// The columns a name record is read back with, in `row_to_name_record` order.
const NAME_COLUMNS: &str = "record_id, schema_version, normalized_name, original_name, record_type, \
     target, target_scheme, target_canonical, ttl_secs, by_agent_id, origin_agent_id, timestamp, signature";

/// Store a record this machine authored, with no transport origin.
///
/// `av name` has no peer behind it, so `origin_agent_id` stays `NULL` and agent
/// blocks never touch it.
pub fn insert(conn: &Connection, nr: &NameRecord) -> SqlResult<()> {
    insert_with_origin(conn, nr, None)
}

/// Store a record received from a peer, recording where it actually came from.
///
/// `origin` must be the id the transport authenticated, never the id in the
/// record's own payload. Passing `nr.by_agent_id` here would defeat the entire
/// column: that value is chosen by the sender.
pub fn insert_with_origin(
    conn: &Connection,
    nr: &NameRecord,
    origin: Option<&str>,
) -> SqlResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO name_records
            (record_id, schema_version, normalized_name, original_name, record_type,
             target, target_scheme, target_canonical, ttl_secs, by_agent_id,
             origin_agent_id, timestamp, signature)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
            origin,
            nr.timestamp,
            nr.signature,
        ],
    )?;
    Ok(())
}

pub fn get_by_normalized_name(conn: &Connection, name: &str) -> SqlResult<Vec<NameRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {NAME_COLUMNS}
         FROM name_records WHERE normalized_name = ?1 ORDER BY timestamp DESC"
    ))?;
    let rows = stmt.query_map(params![name], |row| row_to_name_record(row))?;
    rows.collect()
}

pub fn list_by_scheme(conn: &Connection, scheme: &str) -> SqlResult<Vec<NameRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {NAME_COLUMNS}
         FROM name_records WHERE target_scheme = ?1 ORDER BY timestamp DESC"
    ))?;
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

/// Match a name record against a resource location, allowing for the
/// `ant://` / `autonomi://` scheme alias and for either column holding the
/// canonical form.
///
/// A resource is addressed by a URI but a name record may store the URI in
/// `target` or in `target_canonical` depending on whether it was written
/// locally or received from a peer, so both columns are checked against both
/// spellings. Returns the parameters for `?1`/`?2` of the matching queries.
fn target_variants(location: &str) -> (String, String) {
    let alternate = if let Some(rest) = location.strip_prefix("autonomi://") {
        format!("ant://{rest}")
    } else if let Some(rest) = location.strip_prefix("ant://") {
        format!("autonomi://{rest}")
    } else {
        location.to_string()
    };
    (location.to_string(), alternate)
}

/// How many name records point at `location`.
///
/// Used to tell the operator what a block-and-delete is about to take with it,
/// rather than discovering the gap afterwards.
pub fn count_by_target(conn: &Connection, location: &str) -> SqlResult<usize> {
    let (a, b) = target_variants(location);
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM name_records
         WHERE target = ?1 OR target_canonical = ?1 OR target = ?2 OR target_canonical = ?2",
        params![a, b],
        |row| row.get(0),
    )?;
    Ok(n.max(0) as usize)
}

/// Delete every name record pointing at `location`, returning how many went.
///
/// A name claim is a pointer to a resource, so removing the resource leaves a
/// claim that can only ever resolve to something the operator deleted. Dropping
/// the claim keeps the two consistent and closes the gap where the name is
/// still locally known but its target is gone.
pub fn delete_by_target(conn: &Connection, location: &str) -> SqlResult<usize> {
    let (a, b) = target_variants(location);
    let n = conn.execute(
        "DELETE FROM name_records
         WHERE target = ?1 OR target_canonical = ?1 OR target = ?2 OR target_canonical = ?2",
        params![a, b],
    )?;
    Ok(n)
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
        origin_agent_id: row.get(10)?,
        timestamp: row.get(11)?,
        signature: row.get(12)?,
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
