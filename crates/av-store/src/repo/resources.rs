use av_core::types::{ResourceDescriptor, ResourceKind};
use rusqlite::{params, Connection, Result as SqlResult};

pub fn insert(conn: &Connection, r: &ResourceDescriptor) -> SqlResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO resources
            (id, kind, location, location_scheme, location_canonical,
             mime_type, filename, metadata_json, description_text, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            r.id,
            kind_to_str(&r.kind),
            r.location,
            r.location_scheme,
            r.location_canonical,
            r.mime_type,
            r.filename,
            r.metadata_json.to_string(),
            r.description_text,
            r.created_at,
        ],
    )?;
    Ok(())
}

pub fn get_by_location(conn: &Connection, location: &str) -> SqlResult<Option<ResourceDescriptor>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, location, location_scheme, location_canonical,
                mime_type, filename, metadata_json, description_text, created_at
         FROM resources WHERE location = ?1
         ORDER BY created_at DESC LIMIT 1",
    )?;
    let mut rows = stmt.query(params![location])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_resource(row)?))
    } else {
        Ok(None)
    }
}

pub fn get(conn: &Connection, id: &str) -> SqlResult<Option<ResourceDescriptor>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, location, location_scheme, location_canonical,
                mime_type, filename, metadata_json, description_text, created_at
         FROM resources WHERE id = ?1",
    )?;
    let mut rows = stmt.query(params![id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_resource(row)?))
    } else {
        Ok(None)
    }
}

pub fn list(conn: &Connection) -> SqlResult<Vec<ResourceDescriptor>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, location, location_scheme, location_canonical,
                mime_type, filename, metadata_json, description_text, created_at
         FROM resources ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], |row| row_to_resource(row))?;
    rows.collect()
}

pub fn delete(conn: &Connection, id: &str) -> SqlResult<()> {
    conn.execute("DELETE FROM resources WHERE id = ?1", params![id])?;
    Ok(())
}

fn row_to_resource(row: &rusqlite::Row<'_>) -> SqlResult<ResourceDescriptor> {
    let kind_str: String = row.get(1)?;
    let metadata_str: String = row.get(7)?;
    Ok(ResourceDescriptor {
        id: row.get(0)?,
        kind: str_to_kind(&kind_str),
        location: row.get(2)?,
        location_scheme: row.get(3)?,
        location_canonical: row.get(4)?,
        mime_type: row.get(5)?,
        filename: row.get(6)?,
        metadata_json: serde_json::from_str(&metadata_str).unwrap_or(serde_json::Value::Null),
        description_text: row.get(8)?,
        created_at: row.get(9)?,
    })
}

fn kind_to_str(k: &ResourceKind) -> &str {
    match k {
        ResourceKind::Text => "text",
        ResourceKind::Image => "image",
        ResourceKind::Audio => "audio",
        ResourceKind::File => "file",
        ResourceKind::Pdf => "pdf",
        ResourceKind::Other(_) => "other",
    }
}

fn str_to_kind(s: &str) -> ResourceKind {
    match s {
        "text" => ResourceKind::Text,
        "image" => ResourceKind::Image,
        "audio" => ResourceKind::Audio,
        "file" => ResourceKind::File,
        "pdf" => ResourceKind::Pdf,
        other => ResourceKind::Other(other.to_owned()),
    }
}
