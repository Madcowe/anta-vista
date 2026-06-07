use av_core::types::{EmbeddingProfile, EmbeddingRecord};
use rusqlite::{Connection, Result as SqlResult, params};

pub fn insert_profile(conn: &Connection, profile_id: &str, p: &EmbeddingProfile) -> SqlResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO embedding_profiles
            (profile_id, model_id, model_version, dim, normalized, preproc_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            profile_id,
            p.model_id,
            p.model_version,
            p.dim as i64,
            p.normalized as i64,
            p.preproc_version,
        ],
    )?;
    Ok(())
}

pub fn get_profile(conn: &Connection, profile_id: &str) -> SqlResult<Option<EmbeddingProfile>> {
    let mut stmt = conn.prepare(
        "SELECT model_id, model_version, dim, normalized, preproc_version
         FROM embedding_profiles WHERE profile_id = ?1",
    )?;
    let mut rows = stmt.query(params![profile_id])?;
    if let Some(row) = rows.next()? {
        let normalized_int: i64 = row.get(3)?;
        Ok(Some(EmbeddingProfile {
            model_id: row.get(0)?,
            model_version: row.get(1)?,
            dim: row.get::<_, i64>(2)? as u16,
            normalized: normalized_int != 0,
            preproc_version: row.get(4)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn insert(conn: &Connection, e: &EmbeddingRecord) -> SqlResult<()> {
    let vector_json = serde_json::to_string(&e.vector)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
    conn.execute(
        "INSERT OR REPLACE INTO embeddings
            (resource_id, profile_id, vector_json, l2_norm, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            e.resource_id,
            e.profile_id,
            vector_json,
            e.l2_norm as f64,
            e.created_at,
        ],
    )?;
    Ok(())
}

pub fn get(
    conn: &Connection,
    resource_id: &str,
    profile_id: &str,
) -> SqlResult<Option<EmbeddingRecord>> {
    let mut stmt = conn.prepare(
        "SELECT resource_id, profile_id, vector_json, l2_norm, created_at
         FROM embeddings WHERE resource_id = ?1 AND profile_id = ?2",
    )?;
    let mut rows = stmt.query(params![resource_id, profile_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_embedding(row)?))
    } else {
        Ok(None)
    }
}

pub fn delete(conn: &Connection, resource_id: &str, profile_id: &str) -> SqlResult<()> {
    conn.execute(
        "DELETE FROM embeddings WHERE resource_id = ?1 AND profile_id = ?2",
        params![resource_id, profile_id],
    )?;
    Ok(())
}

fn row_to_embedding(row: &rusqlite::Row<'_>) -> SqlResult<EmbeddingRecord> {
    let vector_json: String = row.get(2)?;
    let vector: Vec<f32> = serde_json::from_str(&vector_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(EmbeddingRecord {
        resource_id: row.get(0)?,
        profile_id: row.get(1)?,
        vector,
        l2_norm: row.get::<_, f64>(3)? as f32,
        created_at: row.get(4)?,
    })
}
