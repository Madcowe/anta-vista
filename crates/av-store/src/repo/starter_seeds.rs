//! The per-location record that a bundled starter has been settled.
//!
//! See migration [`008_starter_seeds`](../../schema/constant.MIGRATION_008_STARTER_SEEDS.html)
//! for why this is a table of its own: it has to remember a row that no
//! longer exists (a purged starter) and a row that was never ours (one the
//! user indexed first).

use rusqlite::{Connection, Result as SqlResult, params};

/// Whether this location's starter is already settled — seeded, or adopted
/// from the user's own copy.
pub fn is_seeded(conn: &Connection, location: &str) -> SqlResult<bool> {
    let mut stmt = conn.prepare("SELECT 1 FROM starter_seeds WHERE location = ?1")?;
    let mut rows = stmt.query(params![location])?;
    match rows.next()? {
        Some(_) => Ok(true),
        None => Ok(false),
    }
}

/// Remember that this location's starter is settled. Idempotent: a location
/// already marked keeps its original `seeded_at`.
pub fn mark_seeded(conn: &Connection, location: &str, seeded_at: i64) -> SqlResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO starter_seeds (location, seeded_at) VALUES (?1, ?2)",
        params![location, seeded_at],
    )?;
    Ok(())
}
