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
    /// The `subject_kind` exactly as stored.
    ///
    /// Kept alongside the parsed kind because `BlockKind` cannot represent a
    /// value it does not know, and `subject_kind` has no CHECK constraint: a
    /// row written by a newer build, or a corrupted database, would otherwise
    /// be silently reported as a `resource` block. Callers that *display* a
    /// subject should use this, and reserve `subject_kind` for matching.
    pub subject_kind_raw: String,
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

/// Is the resource at this location blocked?
///
/// For the paths that are handed a *location* rather than a `resources.id` —
/// fetching an Autonomi record by address, for instance. Those cannot use
/// [`is_blocked`], and they must not be tempted into `resources::get_by_location`
/// to manufacture an id: that matches `location` alone, so it misses every
/// Autonomi row, because a record's stored `location` keeps the author's
/// `/name` segment while a caller holds the canonical address. A block checked
/// that way silently never matches, which reads as "the block is working".
///
/// Two sources, because neither is sufficient on its own:
///
/// * `blocked_locations` is the record left by [`record_blocked_locations`] at
///   block time. This is the one that works in practice, and the reason it has to
///   exist: `av block` **deletes the local copy** by default, so the join below
///   has nothing left to join against by the time anyone asks. A guard that only
///   looked at `resources` would pass a test that inserted a block and kept the
///   row, and fail in exactly the situation the block was made for.
/// * The join on **either** spelling covers what the table cannot: a block imposed
///   before this table existed, and a `--keep-local` block where the resource is
///   still there. The spelling matters for the same reason
///   `resources::find_filename` needs it.
///
/// Resource blocks only. A *name* block covers the claim, not the location it
/// resolves to — `av_index::naming::retain_unblocked_targets` is the other
/// direction, suppressing names that point at a blocked resource — so a name
/// block deliberately does not refuse a direct fetch of that location. Agent
/// blocks do not apply either: a fetch uses this node's own identity and
/// consults no peer that could be blocked.
///
/// Fails closed. Both reads propagate, because "we could not check" is not
/// evidence that nothing is blocked, and this is a serving path.
pub fn is_location_blocked(conn: &Connection, location: &str) -> SqlResult<bool> {
    let recorded: i64 = conn.query_row(
        "SELECT COUNT(*) FROM blocked_locations WHERE location = ?1",
        params![location],
        |row| row.get(0),
    )?;
    if recorded > 0 {
        return Ok(true);
    }

    let joined: i64 = conn.query_row(
        "SELECT COUNT(*) FROM blocks b
           JOIN resources r ON r.id = b.subject_id
         WHERE b.subject_kind = ?1
           AND (r.location = ?2 OR r.location_canonical = ?2)",
        params![BlockKind::Resource.as_str(), location],
        |row| row.get(0),
    )?;
    Ok(joined > 0)
}

/// Remember every spelling a blocked resource was held under.
///
/// Must be called **before** the resource row is deleted, because that row is the
/// only place the location is written down. Callers that skip it get a block that
/// `is_location_blocked` cannot see for a location-based fetch, which is the
/// failure this function exists to prevent.
///
/// Idempotent, and a no-op for a resource this machine does not hold. That is not
/// a gap: blocking an id we never held cannot be matched against a location by
/// anyone either, and pretending otherwise would make "blocked" mean "blocked
/// somewhere we happen to know about".
pub fn record_blocked_locations(conn: &Connection, subject_id: &str) -> SqlResult<usize> {
    let mut stmt = conn.prepare(
        "SELECT location, location_canonical FROM resources
          WHERE id = ?1
            AND (location IS NOT NULL OR location_canonical IS NOT NULL)",
    )?;
    let spellings = stmt
        .query_map(params![subject_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })?
        .collect::<SqlResult<Vec<_>>>()?;

    let mut recorded = 0usize;
    let mut seen = HashSet::new();
    for (location, canonical) in spellings {
        for spelling in [location, canonical].into_iter().flatten() {
            // One record indexed under two paths names the same location twice,
            // and `INSERT OR REPLACE` on the primary key makes that a no-op
            // rather than a duplicate.
            if !seen.insert(spelling.clone()) {
                continue;
            }
            conn.execute(
                "INSERT OR REPLACE INTO blocked_locations (resource_id, location) VALUES (?1, ?2)",
                params![subject_id, spelling],
            )?;
            recorded += 1;
        }
    }
    Ok(recorded)
}

/// Drop the recorded spellings for a resource that has been unblocked.
///
/// Called by `av unblock`, because otherwise unblocking would leave the location
/// table still refusing a fetch — a block the user cannot lift through the API,
/// which is worse than the bug this replaces.
///
/// Deletes by the recorded resource id rather than by looking the locations up
/// again, because the resource row is gone in exactly the case that matters: the
/// block deleted it. Re-reading `resources` here would always find nothing, and
/// the refusal would outlive the unblock.
pub fn forget_blocked_locations(conn: &Connection, subject_id: &str) -> SqlResult<usize> {
    Ok(conn.execute(
        "DELETE FROM blocked_locations WHERE resource_id = ?1",
        params![subject_id],
    )?)
}

/// Resource ids that have at least one recorded location, for a `--kind` clear.
///
/// The table carries no `subject_kind`, because every row in it is a resource
/// block by construction — `record_blocked_locations` is only called for one.
/// A `--kind resource` clear therefore has to find the ids to clear from the
/// table, and this is that read.
pub fn resource_ids_with_blocked_locations(conn: &Connection) -> SqlResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT resource_id FROM blocked_locations")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    // Propagates rather than skipping: a row we cannot read is a location whose
    // block this clear would leave behind.
    for id in rows {
        out.push(id?);
    }
    Ok(out)
}

/// Forget every recorded spelling, for `av unblock --all`.
pub fn forget_all_blocked_locations(conn: &Connection) -> SqlResult<usize> {
    Ok(conn.execute("DELETE FROM blocked_locations", [])?)
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
            subject_kind_raw: kind_str,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::resources;
    use av_core::types::{ResourceDescriptor, ResourceKind};

    const HEX: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

    /// A record indexed at `autonomi://<hex>/welcome.md`, which is the shape
    /// every Autonomi row has: `location` keeps the author's path segment and
    /// `location_canonical` is the bare address.
    fn autonomi_record(id: &str) -> ResourceDescriptor {
        ResourceDescriptor {
            id: id.to_string(),
            kind: ResourceKind::Text,
            location: format!("autonomi://{HEX}/welcome.md"),
            location_scheme: Some("autonomi".to_string()),
            location_canonical: Some(format!("autonomi://{HEX}")),
            mime_type: "text/markdown".to_string(),
            filename: Some("welcome.md".to_string()),
            metadata_json: serde_json::json!({}),
            description_text: String::new(),
            created_at: 1_700_000_000,
        }
    }

    fn block_resource(conn: &Connection, id: &str) {
        add(conn, BlockKind::Resource, id, None, 1_700_000_000, "test").expect("block");
    }

    #[test]
    fn a_location_is_blocked_when_the_resource_behind_it_is() {
        let conn = crate::open_in_memory().expect("open db");
        let r = autonomi_record("r1");
        resources::insert(&conn, &r).expect("insert");
        block_resource(&conn, &r.id);

        assert!(is_location_blocked(&conn, &r.location).expect("query"));
    }

    /// The half of the join that does the real work.
    ///
    /// A blocked Autonomi record is stored under its full path, and every caller
    /// that fetches by address holds only the canonical form — so the join's
    /// second predicate is the one that decides whether blocking an Autonomi
    /// record does anything at all. `resources::get_by_location` cannot be used
    /// here precisely because it matches `location` alone and would answer
    /// "not blocked" for every one of them.
    #[test]
    fn the_canonical_address_finds_the_block_too() {
        let conn = crate::open_in_memory().expect("open db");
        let r = autonomi_record("r1");
        resources::insert(&conn, &r).expect("insert");
        block_resource(&conn, &r.id);

        assert!(
            is_location_blocked(&conn, &format!("autonomi://{HEX}")).expect("query"),
            "a fetch by address must find the block on the record it points at"
        );
    }

    /// The bug this table exists for.
    ///
    /// `av block` deletes the local copy by default, so by the time a fetch route
    /// asks whether a location is blocked the row the join needed is *gone*. A
    /// guard written against `resources` alone therefore answers "not blocked"
    /// for exactly the resources a user blocked, and the fetch proceeds — which
    /// reads as the block being ignored rather than as the guard being unable to
    /// find it. The location has to be written down before the delete.
    #[test]
    fn a_location_stays_blocked_after_the_blocked_resource_is_deleted() {
        let conn = crate::open_in_memory().expect("open db");
        let r = autonomi_record("r1");
        resources::insert(&conn, &r).expect("insert");

        // The order `cmd::block::block_subject` uses.
        block_resource(&conn, &r.id);
        record_blocked_locations(&conn, &r.id).expect("record");
        delete_resource_cascade_with_names(&conn, &r.id).expect("delete");

        // Both spellings, and with no `resources` row left to join against.
        for spelling in [&r.location, &r.location_canonical.clone().unwrap()] {
            assert!(
                is_location_blocked(&conn, spelling).expect("query"),
                "{spelling} must stay blocked with the resource row deleted"
            );
        }
        assert!(
            resources::get(&conn, &r.id).expect("read").is_none(),
            "this test is only meaningful if the resource is really gone"
        );
    }

    /// Unblocking has to clear the recorded location even though it cannot look
    /// it up: the resource row is gone in the case that matters, so a clear that
    /// re-reads `resources` finds nothing and the refusal outlives the unblock.
    /// A block the user cannot lift through the API is worse than no block at all.
    #[test]
    fn unblocking_clears_the_recorded_location_without_the_resource_row() {
        let conn = crate::open_in_memory().expect("open db");
        let r = autonomi_record("r1");
        resources::insert(&conn, &r).expect("insert");
        block_resource(&conn, &r.id);
        record_blocked_locations(&conn, &r.id).expect("record");
        delete_resource_cascade_with_names(&conn, &r.id).expect("delete");

        forget_blocked_locations(&conn, &r.id).expect("forget");
        remove(&conn, BlockKind::Resource, &r.id).expect("unblock");

        assert!(!is_location_blocked(&conn, &r.location).expect("query"));
        assert!(!is_location_blocked(&conn, &format!("autonomi://{HEX}")).expect("query"));
    }

    /// `unblock --all` has to clear them too, and the same test shape proves the
    /// `--kind resource` path, which cannot blanket-delete the table.
    #[test]
    fn a_bulk_unblock_clears_recorded_locations() {
        let conn = crate::open_in_memory().expect("open db");
        let r = autonomi_record("r1");
        resources::insert(&conn, &r).expect("insert");
        block_resource(&conn, &r.id);
        record_blocked_locations(&conn, &r.id).expect("record");
        delete_resource_cascade_with_names(&conn, &r.id).expect("delete");

        assert_eq!(
            resource_ids_with_blocked_locations(&conn).expect("ids"),
            vec![r.id.clone()],
            "the recorded id is what makes a --kind clear possible at all"
        );

        for id in resource_ids_with_blocked_locations(&conn).expect("ids") {
            forget_blocked_locations(&conn, &id).expect("forget");
        }
        clear(&conn, None).expect("clear all");
        assert!(!is_location_blocked(&conn, &r.location).expect("query"));

        // And the blanket form, for `unblock --all`.
        block_resource(&conn, &r.id);
        record_blocked_locations(&conn, &r.id).expect("record");
        forget_all_blocked_locations(&conn).expect("forget all");
        assert!(!is_location_blocked(&conn, &r.location).expect("query"));
    }

    /// One address, two paths, two resource ids — which is what indexing a record
    /// under both its own name and a bare address actually produces, since `id`
    /// is the content hash and the two paths hash the same bytes to the same id
    /// only if the paths are part of the hash.
    ///
    /// Blocking one of them has to refuse the *canonical* address, or the block
    /// has a hole exactly where a user would try next: re-fetch by bare address
    /// and the same record comes back. That is the `location_canonical` predicate
    /// doing its job, and it is why `resources::get_by_location` cannot stand in
    /// for it here.
    #[test]
    fn blocking_one_path_refuses_the_shared_canonical_address() {
        let conn = crate::open_in_memory().expect("open db");
        let named = autonomi_record("r1");
        let bare = ResourceDescriptor {
            id: "r2".to_string(),
            location: format!("autonomi://{HEX}"),
            filename: None,
            ..autonomi_record("r1")
        };
        resources::insert(&conn, &named).expect("insert named");
        resources::insert(&conn, &bare).expect("insert bare");
        assert_ne!(named.id, bare.id, "two rows sharing an address");

        // Only the named row is blocked.
        block_resource(&conn, &named.id);
        record_blocked_locations(&conn, &named.id).expect("record");

        assert!(is_location_blocked(&conn, &named.location).expect("query"));
        assert!(
            is_location_blocked(&conn, &format!("autonomi://{HEX}")).expect("query"),
            "re-fetching by bare address must not walk around the block"
        );

        // Both spellings, and recording twice must not double the rows.
        record_blocked_locations(&conn, &named.id).expect("record again");
        assert_eq!(
            forget_blocked_locations(&conn, &named.id).expect("forget"),
            2,
            "the full path plus the one canonical address"
        );
    }

    /// A block on an id this machine never held records nothing, because there is
    /// no location to attribute to it. Treating that as "blocked everywhere" would
    /// make `av block` on a peer-supplied id suppress the entire address space.
    #[test]
    fn blocking_a_resource_we_do_not_hold_records_no_location() {
        let conn = crate::open_in_memory().expect("open db");
        block_resource(&conn, "never-held");
        assert_eq!(
            record_blocked_locations(&conn, "never-held").expect("record"),
            0
        );
        assert!(!is_location_blocked(&conn, &format!("autonomi://{HEX}")).expect("query"));
    }

    #[test]
    fn an_unblocked_or_absent_resource_is_not_blocked() {
        let conn = crate::open_in_memory().expect("open db");
        let r = autonomi_record("r1");
        resources::insert(&conn, &r).expect("insert");

        assert!(
            !is_location_blocked(&conn, &r.location).expect("query"),
            "held but not blocked"
        );
        assert!(
            !is_location_blocked(&conn, &format!("autonomi://{}", "f".repeat(64))).expect("query"),
            "a location we hold nothing for is not blocked"
        );
    }

    #[test]
    fn a_name_or_agent_block_does_not_block_the_location() {
        // The kinds are independent by design. A name block covers the claim and
        // an agent block covers the peer; neither is a statement about this
        // address, so neither may refuse a direct fetch of it.
        let conn = crate::open_in_memory().expect("open db");
        let r = autonomi_record("r1");
        resources::insert(&conn, &r).expect("insert");

        // Both deliberately reuse the resource id, which is the strongest form of
        // this test: only the `subject_kind` column separates them.
        add(&conn, BlockKind::Name, &r.id, None, 1_700_000_000, "test").expect("name block");
        assert!(!is_location_blocked(&conn, &r.location).expect("query"));

        add(&conn, BlockKind::Agent, &r.id, None, 1_700_000_000, "test").expect("agent block");
        assert!(!is_location_blocked(&conn, &r.location).expect("query"));
    }

    #[test]
    fn a_broken_block_table_refuses_rather_than_serves() {
        // The load-bearing one. `unwrap_or(false)` here would turn a dropped or
        // unreadable `blocks` table into "nothing is blocked", which is the exact
        // outcome a block exists to prevent — and it would look like the feature
        // working, because the fetch would succeed.
        let conn = crate::open_in_memory().expect("open db");
        let r = autonomi_record("r1");
        resources::insert(&conn, &r).expect("insert");
        block_resource(&conn, &r.id);
        assert!(is_location_blocked(&conn, &r.location).expect("query with the table"));

        conn.execute_batch("DROP TABLE blocks").expect("drop");
        assert!(
            is_location_blocked(&conn, &r.location).is_err(),
            "an unreadable block list must not read as \"not blocked\""
        );
    }

    #[test]
    fn lifting_the_block_releases_the_location_again() {
        let conn = crate::open_in_memory().expect("open db");
        let r = autonomi_record("r1");
        resources::insert(&conn, &r).expect("insert");
        block_resource(&conn, &r.id);

        remove(&conn, BlockKind::Resource, &r.id).expect("unblock");
        assert!(
            !is_location_blocked(&conn, &format!("autonomi://{HEX}")).expect("query"),
            "unblock has to actually unblock"
        );
    }
}
