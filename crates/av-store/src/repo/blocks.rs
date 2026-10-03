use rusqlite::{Connection, Result as SqlResult, params};
use std::collections::HashSet;

/// What a block is about. The three kinds are independent: blocking a
/// resource never implies anything about the agent that returned it, and
/// blocking an agent does not condemn anything they returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// A `resources.id` (a SHA-256 content hash).
    Resource,
    /// A `name_records.record_id`.
    Name,
    /// A 64-hex x0x agent id.
    Agent,
}

impl BlockKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockKind::Resource => "resource",
            BlockKind::Name => "name",
            BlockKind::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "resource" => Some(BlockKind::Resource),
            "name" => Some(BlockKind::Name),
            "agent" => Some(BlockKind::Agent),
            _ => None,
        }
    }

    pub const ALL: [BlockKind; 3] = [BlockKind::Resource, BlockKind::Name, BlockKind::Agent];
}

/// One blocked subject, as shown by `av block --list`.
#[derive(Debug, Clone)]
pub struct BlockEntry {
    pub subject_kind: BlockKind,
    pub subject_id: String,
    pub reason: Option<String>,
    pub blocked_at: i64,
    pub source: String,
}

/// Record a block. Returns `false` if this subject was already blocked, so
/// callers can say so honestly rather than reporting a fresh block.
///
/// Re-blocking keeps the original `blocked_at` and does not overwrite the
/// reason: the point of the row is "this is blocked since ...", not a log of
/// every time the operator mentioned it.
pub fn add(
    conn: &Connection,
    kind: BlockKind,
    subject_id: &str,
    reason: Option<&str>,
    blocked_at: i64,
    source: &str,
) -> SqlResult<bool> {
    let changed = conn.execute(
        "INSERT OR IGNORE INTO blocks (subject_kind, subject_id, reason, blocked_at, source)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![kind.as_str(), subject_id, reason, blocked_at, source],
    )?;
    Ok(changed > 0)
}

/// Lift a block. Returns `false` if the subject was not blocked, so callers
/// can distinguish "unblocked" from "was never blocked".
pub fn remove(conn: &Connection, kind: BlockKind, subject_id: &str) -> SqlResult<bool> {
    let changed = conn.execute(
        "DELETE FROM blocks WHERE subject_kind = ?1 AND subject_id = ?2",
        params![kind.as_str(), subject_id],
    )?;
    Ok(changed > 0)
}

pub fn is_blocked(conn: &Connection, kind: BlockKind, subject_id: &str) -> SqlResult<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM blocks WHERE subject_kind = ?1 AND subject_id = ?2",
        params![kind.as_str(), subject_id],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

/// Every blocked id of one kind, for building a filter.
///
/// This is the hot-path read: search iterates every embedded resource, so
/// callers load the set once and test membership rather than issuing a query
/// per candidate. A missing `blocks` table is treated as "nothing blocked" so
/// an older database file cannot break search.
pub fn blocked_ids(conn: &Connection, kind: BlockKind) -> SqlResult<HashSet<String>> {
    // Both the prepare and the row read propagate. Returning an empty set on
    // error would be indistinguishable from "nothing is blocked" and would
    // turn a database problem into a silent un-hiding of every blocked subject,
    // which is the one outcome a block exists to prevent. Callers on the
    // serving path already propagate this error rather than defaulting.
    let mut stmt = conn.prepare("SELECT subject_id FROM blocks WHERE subject_kind = ?1")?;
    let rows = stmt.query_map(params![kind.as_str()], |row| row.get::<_, String>(0))?;
    let mut out = HashSet::new();
    for id in rows {
        // A row that cannot be read is an error, not a row to skip: dropping it
        // here would re-admit exactly the one subject whose block we failed to
        // read, and the failure would be invisible.
        out.insert(id?);
    }
    Ok(out)
}

pub fn list_all(conn: &Connection) -> SqlResult<Vec<BlockEntry>> {
    let mut stmt = conn.prepare(
        "SELECT subject_kind, subject_id, reason, blocked_at, source
         FROM blocks ORDER BY subject_kind, blocked_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        let kind_str: String = row.get(0)?;
        Ok(BlockEntry {
            subject_kind: BlockKind::parse(&kind_str).unwrap_or(BlockKind::Resource),
            subject_id: row.get(1)?,
            reason: row.get(2)?,
            blocked_at: row.get(3)?,
            source: row.get(4)?,
        })
    })?;
    rows.collect()
}

/// Remove every block of one kind, or all of them when `kind` is `None`.
/// Returns the number of rows removed.
pub fn clear(conn: &Connection, kind: Option<BlockKind>) -> SqlResult<usize> {
    match kind {
        Some(k) => conn.execute(
            "DELETE FROM blocks WHERE subject_kind = ?1",
            params![k.as_str()],
        ),
        None => conn.execute("DELETE FROM blocks", []),
    }
}

/// How many subjects of each kind are blocked, for `av status`.
///
/// Returns an empty map if the table is missing, so status reporting cannot
/// itself become the thing that breaks.
pub fn counts_by_kind(conn: &Connection) -> SqlResult<Vec<(BlockKind, i64)>> {
    let mut stmt =
        match conn.prepare("SELECT subject_kind, COUNT(*) FROM blocks GROUP BY subject_kind") {
            Ok(s) => s,
            Err(_) => return Ok(vec![]),
        };
    let rows = stmt.query_map([], |row| {
        let kind_str: String = row.get(0)?;
        let n: i64 = row.get(1)?;
        Ok((
            BlockKind::parse(&kind_str).unwrap_or(BlockKind::Resource),
            n,
        ))
    })?;
    rows.collect()
}

/// Delete a resource row, relying on the `ON DELETE CASCADE` foreign keys that
/// `av_store` enables on every connection (`embeddings`, `feedback_events` and
/// `relevance_judgments` all reference `resources(id)`).
///
/// Returns `false` if there was no such row, so callers do not claim to have
/// removed something that was never there.
pub fn delete_resource_cascade(conn: &Connection, resource_id: &str) -> SqlResult<bool> {
    let n = conn.execute("DELETE FROM resources WHERE id = ?1", params![resource_id])?;
    Ok(n > 0)
}

/// What `delete_resource_cascade_with_names` removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResourceCascade {
    /// Whether a `resources` row actually went away.
    pub resource_removed: bool,
    /// Name records that pointed at the resource, dropped with it.
    pub names_removed: usize,
}

/// Delete a resource, the rows that cascade from it, and any local name claim
/// pointing at it.
///
/// The name rows are not foreign keys — a claim stores a URI, not a `resources`
/// id — so they would otherwise be left behind, still answering to a name whose
/// only local resolution is gone. Once this returns, a peer that re-supplies
/// that name is no longer the local gap it was.
///
/// The location is read before the delete, and the whole thing runs in one
/// transaction so a failure cannot leave a resource gone but its claims intact.
pub fn delete_resource_cascade_with_names(
    conn: &Connection,
    resource_id: &str,
) -> SqlResult<ResourceCascade> {
    // `BEGIN IMMEDIATE`, not a deferred transaction: this always writes, and a
    // deferred transaction that starts by reading and later upgrades to a write
    // fails with SQLITE_BUSY_SNAPSHOT if any other connection committed in
    // between. The web server holds a connection per request, so that window is
    // routinely open. `busy_timeout` is set on every connection in
    // `av_store::db`; without it, taking the write lock up front would fail
    // immediately instead of waiting.
    conn.execute_batch("BEGIN IMMEDIATE")?;

    let outcome = (|| -> SqlResult<ResourceCascade> {
        let location: Option<String> = conn
            .query_row(
                "SELECT location FROM resources WHERE id = ?1",
                params![resource_id],
                |row| row.get(0),
            )
            .ok();

        let names_removed = match location.as_deref() {
            Some(loc) => crate::repo::names::delete_by_target(conn, loc)?,
            None => 0,
        };

        let n = conn.execute("DELETE FROM resources WHERE id = ?1", params![resource_id])?;
        Ok(ResourceCascade {
            resource_removed: n > 0,
            names_removed,
        })
    })();

    match outcome {
        Ok(cascade) => {
            conn.execute_batch("COMMIT")?;
            Ok(cascade)
        }
        Err(e) => {
            // Never leave a transaction open on a connection that is handed back
            // to a per-request pool.
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
