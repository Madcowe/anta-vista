use rusqlite::{Connection, Result as SqlResult};
use std::path::Path;

pub fn open(path: &Path) -> SqlResult<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(path)?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Open an in-memory database (for tests).
pub fn open_in_memory() -> SqlResult<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         PRAGMA synchronous=NORMAL;",
    )
}

fn migrate(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(crate::schema::CREATE_APPLIED_MIGRATIONS)?;

    let migrations: &[(&str, &str)] = &[
        ("001_initial", crate::schema::MIGRATION_001_INITIAL),
        ("002_relevance", crate::schema::MIGRATION_002_RELEVANCE),
    ];

    for (name, sql) in migrations {
        let already_applied: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM applied_migrations WHERE name = ?1",
                rusqlite::params![name],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| n > 0)?;

        if !already_applied {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO applied_migrations (name, applied_at) VALUES (?1, strftime('%s','now'))",
                rusqlite::params![name],
            )?;
        }
    }
    Ok(())
}
