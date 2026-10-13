use av_core::types::{ResourceDescriptor, ResourceKind};
use rusqlite::{Connection, Result as SqlResult, params};

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

/// The name we stored for a record, if we hold one.
///
/// Matches on **either** spelling because the two callers want opposite things
/// from the column and neither existing read is right for this one.
/// `get_by_location` matches `location` alone, which is the full URI including
/// any `/name` segment, so it misses a caller holding the canonical address —
/// and widening *it* is not an option, because it feeds
/// `av_index::naming::resolve_target_resource`, the name-to-resource bridge
/// that resource blocks depend on.
///
/// `ORDER BY created_at DESC` because `location_canonical` is not unique: the
/// same record indexed under two different paths is two rows sharing one
/// canonical address, and the newest is as good an answer as any.
///
/// Returns `Err` rather than `Ok(None)` for a broken table, because "we hold no
/// name" and "we could not ask" must not collapse — the same fail-closed rule
/// the block readers follow.
pub fn find_filename(conn: &Connection, location: &str) -> SqlResult<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT filename FROM resources
         WHERE filename IS NOT NULL
           AND (location = ?1 OR location_canonical = ?1)
         ORDER BY created_at DESC LIMIT 1",
    )?;
    let mut rows = stmt.query(params![location])?;
    match rows.next()? {
        Some(row) => Ok(row.get(0)?),
        None => Ok(None),
    }
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
        ResourceKind::WatchList => "watchlist",
        ResourceKind::Manifest => "manifest",
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
        "watchlist" => ResourceKind::WatchList,
        "manifest" => ResourceKind::Manifest,
        other => ResourceKind::Other(other.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use av_core::types::ResourceDescriptor;

    #[test]
    fn kind_roundtrip_all_variants() {
        let kinds = [
            ResourceKind::Text,
            ResourceKind::Image,
            ResourceKind::Audio,
            ResourceKind::File,
            ResourceKind::Pdf,
            ResourceKind::WatchList,
            ResourceKind::Manifest,
        ];
        for kind in kinds {
            assert_eq!(str_to_kind(kind_to_str(&kind)), kind);
        }
        // Other(_) collapses to the "other" column value.
        assert_eq!(kind_to_str(&ResourceKind::Other("x".into())), "other");
        assert_eq!(
            str_to_kind("other"),
            ResourceKind::Other("other".to_owned())
        );
    }

    fn base_descriptor() -> ResourceDescriptor {
        ResourceDescriptor {
            id: "r1".to_string(),
            kind: ResourceKind::WatchList,
            location: "file:///tmp/lib.watch-list".to_string(),
            location_scheme: Some("file".to_string()),
            location_canonical: None,
            mime_type: "application/x-watch-list".to_string(),
            filename: Some("lib.watch-list".to_string()),
            metadata_json: serde_json::json!({}),
            description_text: "a demo watch list".to_string(),
            created_at: 1_700_000_000,
        }
    }

    #[test]
    fn watchlist_kind_serializes_to_column() {
        assert_eq!(kind_to_str(&ResourceKind::WatchList), "watchlist");
    }

    #[test]
    fn str_to_kind_unknown_becomes_other() {
        assert_eq!(str_to_kind("watchlist"), ResourceKind::WatchList);
        assert_eq!(str_to_kind("manifest"), ResourceKind::Manifest);
        assert_eq!(
            str_to_kind("mystery"),
            ResourceKind::Other("mystery".to_owned())
        );
    }

    #[test]
    fn insert_reads_watchlist_roundtrip() {
        let conn = crate::open_in_memory().expect("open db");
        let r = base_descriptor();
        insert(&conn, &r).expect("insert");
        let got = get(&conn, &r.id).expect("get").expect("present");
        assert_eq!(got.kind, ResourceKind::WatchList);
        assert_eq!(got.mime_type, "application/x-watch-list");
    }

    #[test]
    fn descriptor_roundtrips_through_db() {
        let conn = crate::open_in_memory().expect("open db");
        let r = base_descriptor();
        insert(&conn, &r).expect("insert");
        let got = get(&conn, &r.id).expect("get").expect("present");
        assert_eq!(got, r);
    }

    /// A record indexed at `autonomi://<hex>/welcome.md`.
    fn autonomi_record(id: &str, path: &str, created_at: i64) -> ResourceDescriptor {
        let hex = "a".repeat(64);
        ResourceDescriptor {
            id: id.to_string(),
            kind: ResourceKind::Text,
            location: format!("autonomi://{hex}/{path}"),
            location_scheme: Some("autonomi".to_string()),
            location_canonical: Some(format!("autonomi://{hex}")),
            mime_type: "text/markdown".to_string(),
            filename: Some(path.to_string()),
            metadata_json: serde_json::json!({}),
            description_text: String::new(),
            created_at,
        }
    }

    #[test]
    fn find_filename_answers_from_either_spelling_of_the_location() {
        // The point of the second predicate. An Autonomi record's canonical
        // location is the address alone, so a caller holding the canonical form
        // — which is what a search result carries, and therefore what the page
        // echoes back — misses the row entirely under `location = ?1`. That is
        // how `welcome.md` became `52af8a181c6171f8.md`.
        let conn = crate::open_in_memory().expect("open db");
        let r = autonomi_record("r1", "welcome.md", 1_700_000_000);
        insert(&conn, &r).expect("insert");

        assert_eq!(
            find_filename(&conn, &r.location).expect("query").as_deref(),
            Some("welcome.md"),
            "the full location still matches"
        );
        assert_eq!(
            find_filename(&conn, r.location_canonical.as_deref().unwrap())
                .expect("query")
                .as_deref(),
            Some("welcome.md"),
            "and so does the canonical address"
        );
    }

    #[test]
    fn find_filename_prefers_the_newest_when_a_canonical_is_shared() {
        // `location_canonical` is not unique: one record indexed under two paths
        // is two rows sharing one address. The pick has to be deterministic, and
        // a LIMIT 1 without ORDER BY would not be.
        let conn = crate::open_in_memory().expect("open db");
        let older = autonomi_record("r1", "first.md", 1_700_000_000);
        let newer = autonomi_record("r2", "second.md", 1_700_000_100);
        insert(&conn, &older).expect("insert older");
        insert(&conn, &newer).expect("insert newer");

        let canonical = newer.location_canonical.as_deref().unwrap();
        assert_eq!(
            find_filename(&conn, canonical).expect("query").as_deref(),
            Some("second.md")
        );
    }

    #[test]
    fn find_filename_is_absent_for_an_unknown_location_or_a_nameless_row() {
        let conn = crate::open_in_memory().expect("open db");
        assert_eq!(
            find_filename(&conn, "autonomi://nope").expect("query"),
            None,
            "we hold nothing for this"
        );

        // An http(s) row carries no filename, and must read as "no name" rather
        // than as an empty string the caller would have to filter out.
        let mut nameless = base_descriptor();
        nameless.filename = None;
        insert(&conn, &nameless).expect("insert");
        assert_eq!(
            find_filename(&conn, &nameless.location).expect("query"),
            None
        );
    }

    #[test]
    fn find_filename_propagates_a_broken_table() {
        // Fail-closed is not free here either: a caller that cannot ask must not
        // read as "there is no name", because that is indistinguishable from a
        // record that genuinely has none. Both are safe to fall back from, but
        // only one of them is true, and swallowing the error hides which.
        let conn = crate::open_in_memory().expect("open db");
        conn.execute_batch("DROP TABLE resources").expect("drop");
        assert!(
            find_filename(&conn, "autonomi://whatever").is_err(),
            "a missing table must not read as \"no name\""
        );
    }
}
