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
    // `busy_timeout` is not optional here. The web server opens a connection per
    // request and the responder opens its own, so writes genuinely overlap, and
    // SQLite's default of 0 turns a momentary overlap into an immediate
    // `SQLITE_BUSY` for whichever request loses the race. Five seconds is well
    // past any real write and well inside a request timeout.
    //
    // It goes first on purpose. `busy_timeout` is a per-connection setting, so
    // every statement before it in the batch would run with the default of 0.
    conn.execute_batch(
        "PRAGMA busy_timeout=5000;
         PRAGMA foreign_keys=ON;
         PRAGMA synchronous=NORMAL;",
    )?;
    ensure_wal(conn)
}

/// Put the database in WAL mode, tolerating a concurrent opener.
///
/// Changing the journal mode needs a lock that no other connection can grant,
/// and SQLite answers `SQLITE_BUSY` there *without consulting `busy_timeout`*.
/// A burst of connections opening a brand-new database therefore used to fail
/// with "database is locked" on whichever thread lost that race, which made
/// `open` fail intermittently and took the web server down with it.
///
/// WAL is a persistent property of the file, not of the connection, so losing
/// the race costs nothing: whoever gets there first has already achieved the
/// goal we wanted. Re-read the mode and accept it if somebody else set it. The
/// short bounded retry only covers the window where the winner has taken the
/// lock but not yet committed, and stays far inside the `busy_timeout` budget
/// so it can never be the thing that makes a request slow.
fn ensure_wal(conn: &Connection) -> SqlResult<()> {
    if journal_mode_is_wal(conn)? {
        return Ok(());
    }
    const ATTEMPTS: u32 = 50;
    for attempt in 0..ATTEMPTS {
        match conn.execute_batch("PRAGMA journal_mode=WAL;") {
            Ok(()) => return Ok(()),
            Err(e) => {
                if journal_mode_is_wal(conn)? {
                    return Ok(());
                }
                if attempt + 1 == ATTEMPTS {
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    Ok(())
}

fn journal_mode_is_wal(conn: &Connection) -> SqlResult<bool> {
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    Ok(mode.eq_ignore_ascii_case("wal"))
}

fn migrate(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(crate::schema::CREATE_APPLIED_MIGRATIONS)?;

    for (name, sql) in crate::schema::MIGRATIONS {
        // Fast path: an already-migrated database does no locking at all. This
        // matters because `open` runs per web request.
        if is_applied(conn, name)? {
            continue;
        }

        // The check above is a snapshot, not a claim. `open` is called
        // concurrently on the same file, so several threads can reach a fresh
        // database and all see the same migration as pending. Re-checking under
        // the write lock is what makes that safe: `BEGIN IMMEDIATE` takes the
        // lock before the second check, so the loser's thread blocks until the
        // winner commits and then sees the migration recorded.
        //
        // This is not optional bookkeeping. `CREATE TABLE IF NOT EXISTS` is
        // idempotent, so losing that race was survivable, but `ALTER TABLE ADD
        // COLUMN` (006) is not: the loser used to fail with "duplicate column
        // name" and the web server could not open a connection at all.
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let applied = (|| -> SqlResult<()> {
            if is_applied(conn, name)? {
                return Ok(());
            }
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT OR IGNORE INTO applied_migrations (name, applied_at) VALUES (?1, strftime('%s','now'))",
                rusqlite::params![name],
            )?;
            Ok(())
        })();

        match applied {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(e) => {
                // Roll back so a failed migration does not strand the
                // connection inside an open transaction, which would make every
                // later statement on it fail.
                conn.execute_batch("ROLLBACK").ok();
                return Err(e);
            }
        }
    }
    Ok(())
}

fn is_applied(conn: &Connection, name: &str) -> SqlResult<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM applied_migrations WHERE name = ?1",
        rusqlite::params![name],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}
