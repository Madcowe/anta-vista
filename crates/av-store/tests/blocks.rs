use av_store::repo::blocks::{self, BlockKind};
use av_store::{open, open_in_memory, repo};

const AGENT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const AGENT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const RES_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const RES_B: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn now() -> i64 {
    1_700_000_000
}

#[test]
fn add_then_is_blocked_and_blocked_ids_reflects_it() {
    let conn = open_in_memory().expect("in-memory db");

    assert!(!blocks::is_blocked(&conn, BlockKind::Resource, RES_A).unwrap());

    let fresh = blocks::add(
        &conn,
        BlockKind::Resource,
        RES_A,
        Some("spam"),
        now(),
        "cli",
    )
    .unwrap();
    assert!(fresh, "first block should be reported as newly added");

    assert!(blocks::is_blocked(&conn, BlockKind::Resource, RES_A).unwrap());
    let ids = blocks::blocked_ids(&conn, BlockKind::Resource).unwrap();
    assert!(ids.contains(RES_A), "blocked_ids must include the resource");
}

#[test]
fn re_blocking_is_idempotent_and_preserves_the_original_timestamp() {
    let conn = open_in_memory().expect("in-memory db");

    blocks::add(
        &conn,
        BlockKind::Resource,
        RES_A,
        Some("first reason"),
        100,
        "cli",
    )
    .unwrap();
    let again = blocks::add(
        &conn,
        BlockKind::Resource,
        RES_A,
        Some("second reason"),
        200,
        "web",
    )
    .unwrap();

    assert!(!again, "re-blocking must report that nothing changed");

    let entries = blocks::list_all(&conn).unwrap();
    assert_eq!(entries.len(), 1, "re-blocking must not create a second row");
    assert_eq!(
        entries[0].blocked_at, 100,
        "blocked_at records when the block was first imposed"
    );
    assert_eq!(
        entries[0].reason.as_deref(),
        Some("first reason"),
        "re-blocking must not churn the recorded reason"
    );
}

#[test]
fn remove_reports_whether_the_subject_was_actually_blocked() {
    let conn = open_in_memory().expect("in-memory db");
    blocks::add(&conn, BlockKind::Agent, AGENT_A, None, now(), "cli").unwrap();

    assert!(blocks::remove(&conn, BlockKind::Agent, AGENT_A).unwrap());
    assert!(
        !blocks::remove(&conn, BlockKind::Agent, AGENT_A).unwrap(),
        "removing an already-unblocked subject must report false"
    );
    assert!(!blocks::is_blocked(&conn, BlockKind::Agent, AGENT_A).unwrap());
}

#[test]
fn kinds_are_independent_and_blocked_ids_is_scoped_to_one_kind() {
    let conn = open_in_memory().expect("in-memory db");

    // The same opaque id blocked as a resource and as an agent are separate
    // policies: blocking a resource must never imply blocking a peer.
    blocks::add(&conn, BlockKind::Resource, RES_A, None, now(), "cli").unwrap();
    blocks::add(&conn, BlockKind::Agent, AGENT_A, None, now(), "cli").unwrap();
    blocks::add(&conn, BlockKind::Agent, AGENT_B, None, now(), "cli").unwrap();
    blocks::add(&conn, BlockKind::Name, "name-1", None, now(), "cli").unwrap();

    let resources = blocks::blocked_ids(&conn, BlockKind::Resource).unwrap();
    assert_eq!(resources.len(), 1);
    assert!(resources.contains(RES_A));

    let agents = blocks::blocked_ids(&conn, BlockKind::Agent).unwrap();
    assert_eq!(agents.len(), 2, "the name block must not appear in agents");
    assert!(agents.contains(AGENT_A) && agents.contains(AGENT_B));
    assert!(!agents.contains(RES_A));

    let names = blocks::blocked_ids(&conn, BlockKind::Name).unwrap();
    assert_eq!(names.len(), 1);
    assert!(names.contains("name-1"));

    // Unblocking a resource leaves the agent block intact.
    blocks::remove(&conn, BlockKind::Resource, RES_A).unwrap();
    assert_eq!(
        blocks::blocked_ids(&conn, BlockKind::Agent).unwrap().len(),
        2
    );
}

#[test]
fn clear_can_target_one_kind_or_everything() {
    let conn = open_in_memory().expect("in-memory db");
    blocks::add(&conn, BlockKind::Resource, RES_A, None, now(), "cli").unwrap();
    blocks::add(&conn, BlockKind::Agent, AGENT_A, None, now(), "cli").unwrap();
    blocks::add(&conn, BlockKind::Name, "name-1", None, now(), "cli").unwrap();

    let removed = blocks::clear(&conn, Some(BlockKind::Agent)).unwrap();
    assert_eq!(removed, 1);
    assert!(!blocks::is_blocked(&conn, BlockKind::Agent, AGENT_A).unwrap());
    assert!(blocks::is_blocked(&conn, BlockKind::Resource, RES_A).unwrap());
    assert!(blocks::is_blocked(&conn, BlockKind::Name, "name-1").unwrap());

    let removed = blocks::clear(&conn, None).unwrap();
    assert_eq!(removed, 2);
    assert!(blocks::list_all(&conn).unwrap().is_empty());
}

#[test]
fn the_stored_kind_label_is_preserved_verbatim() {
    // `subject_kind` has no CHECK constraint, so a row written by a newer build
    // (or a corrupted file) can hold a value `BlockKind` cannot represent.
    // Reporting it as a `resource` block would be a lie about local policy, so
    // the raw label travels alongside the parsed kind for callers to display.
    let conn = open_in_memory().expect("in-memory db");
    blocks::add(&conn, BlockKind::Resource, RES_A, None, now(), "cli").unwrap();
    conn.execute(
        "INSERT INTO blocks (subject_kind, subject_id, blocked_at, source)
         VALUES ('watchlist', 'wl-1', 1, 'manual')",
        [],
    )
    .expect("insert a kind this build does not know");

    let entries = blocks::list_all(&conn).unwrap();
    let unknown = entries
        .iter()
        .find(|e| e.subject_id == "wl-1")
        .expect("the unknown-kind row must still be listed");
    assert_eq!(unknown.subject_kind_raw, "watchlist");
    // The parsed kind stays a `BlockKind` for matching, so typed callers are
    // unaffected by the extra field.
    assert_eq!(unknown.subject_kind, BlockKind::Resource);
    assert_eq!(
        entries
            .iter()
            .find(|e| e.subject_id == RES_A)
            .unwrap()
            .subject_kind_raw,
        "resource"
    );
}

#[test]
fn delete_resource_cascade_removes_dependants() {
    use av_core::types::{ResourceDescriptor, ResourceKind};

    let conn = open_in_memory().expect("in-memory db");

    let resource = ResourceDescriptor {
        id: RES_A.to_string(),
        kind: ResourceKind::Text,
        location: "autonomi://example/one".to_string(),
        location_scheme: Some("autonomi".to_string()),
        location_canonical: Some("autonomi://example/one".to_string()),
        mime_type: "text/plain".to_string(),
        filename: None,
        metadata_json: serde_json::json!({}),
        description_text: "a resource".to_string(),
        created_at: now(),
    };
    repo::resources::insert(&conn, &resource).unwrap();

    // Give it a feedback event and a relevance judgment, both of which
    // reference resources(id) ON DELETE CASCADE. The embedding additionally
    // needs its profile to exist, since embeddings.profile_id is a foreign key
    // in its own right.
    conn.execute(
        "INSERT INTO embedding_profiles
           (profile_id, model_id, model_version, dim, normalized, preproc_version)
         VALUES ('p1', 'all-MiniLM-L6-v2', 'v1', 384, 1, 'v1')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO embeddings (resource_id, profile_id, vector_json, l2_norm, created_at)
         VALUES (?1, 'p1', '[1.0]', 1.0, ?2)",
        rusqlite::params![RES_A, now()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO feedback_events
           (feedback_id, query_text, resource_id, by_agent_id, kind, timestamp, signature)
         VALUES ('f1', 'q', ?1, 'agent-x', 'Useful', ?2, x'')",
        rusqlite::params![RES_A, now()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO relevance_judgments (normalized_query, resource_id, score, created_at, updated_at)
         VALUES ('q', ?1, 1.0, ?2, ?2)",
        rusqlite::params![RES_A, now()],
    )
    .unwrap();

    assert!(blocks::delete_resource_cascade(&conn, RES_A).unwrap());
    assert!(
        !blocks::delete_resource_cascade(&conn, RES_A).unwrap(),
        "deleting an absent resource must report false"
    );

    for table in ["embeddings", "feedback_events", "relevance_judgments"] {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "{table} rows should have cascaded away");
    }
}

#[test]
fn blocks_survive_purge_all_which_clears_data_not_policy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("purge.db");
    let conn = open(&db_path).expect("open");

    blocks::add(
        &conn,
        BlockKind::Resource,
        RES_A,
        Some("keep me"),
        now(),
        "cli",
    )
    .unwrap();

    // `av purge --all` deletes these tables; block rows are policy, not data,
    // so they must outlive a data purge.
    for table in [
        "feedback_events",
        "embeddings",
        "resources",
        "name_records",
        "peer_cache",
        "query_cache",
    ] {
        conn.execute(&format!("DELETE FROM {table}"), []).unwrap();
    }

    assert!(
        blocks::is_blocked(&conn, BlockKind::Resource, RES_A).unwrap(),
        "a full data purge must not silently lift blocks"
    );
    let entries = blocks::list_all(&conn).unwrap();
    assert_eq!(entries[0].reason.as_deref(), Some("keep me"));
}

/// Insert a resource with a known location, so the name tests below have
/// something for a name record to point at.
fn insert_resource(conn: &rusqlite::Connection, id: &str, location: &str) {
    use av_core::types::{ResourceDescriptor, ResourceKind};

    repo::resources::insert(
        conn,
        &ResourceDescriptor {
            id: id.to_string(),
            kind: ResourceKind::Text,
            location: location.to_string(),
            location_scheme: Some("autonomi".to_string()),
            location_canonical: Some(location.to_string()),
            mime_type: "text/plain".to_string(),
            filename: None,
            metadata_json: serde_json::json!({}),
            description_text: "a resource".to_string(),
            created_at: now(),
        },
    )
    .unwrap();
}

fn name_record(
    record_id: &str,
    name: &str,
    target: &str,
    canonical: Option<&str>,
) -> av_core::types::NameRecord {
    use av_core::types::NameRecordType;

    av_core::types::NameRecord {
        schema_version: 1,
        record_id: record_id.to_string(),
        normalized_name: name.to_lowercase(),
        original_name: name.to_string(),
        record_type: NameRecordType::Uri,
        target: target.to_string(),
        target_scheme: Some("autonomi".to_string()),
        target_canonical: canonical.map(|c| c.to_string()),
        ttl_secs: 86_400,
        by_agent_id: AGENT_A.to_string(),
        origin_agent_id: None,
        timestamp: now(),
        signature: vec![],
    }
}

#[test]
fn name_claims_are_counted_and_dropped_with_the_resource_they_target() {
    let conn = open_in_memory().expect("in-memory db");
    let location = "autonomi://example/target";
    let other = "autonomi://example/unrelated";

    insert_resource(&conn, RES_A, location);
    insert_resource(&conn, RES_B, other);

    repo::names::insert(&conn, &name_record("n1", "docs", location, None)).unwrap();
    repo::names::insert(&conn, &name_record("n2", "Docs", location, Some(location))).unwrap();
    repo::names::insert(&conn, &name_record("n3", "elsewhere", other, None)).unwrap();

    assert_eq!(
        repo::names::count_by_target(&conn, location).unwrap(),
        2,
        "both claims for this resource must be counted, whichever column holds the target"
    );

    let cascade = blocks::delete_resource_cascade_with_names(&conn, RES_A).unwrap();
    assert!(cascade.resource_removed);
    assert_eq!(
        cascade.names_removed, 2,
        "claims must be reported, not silently dropped"
    );

    assert!(
        repo::names::get_by_normalized_name(&conn, "docs")
            .unwrap()
            .is_empty(),
        "a claim cannot outlive the only thing it resolves to"
    );
    assert_eq!(
        repo::names::get_by_normalized_name(&conn, "elsewhere")
            .unwrap()
            .len(),
        1,
        "an unrelated claim must survive"
    );
}

#[test]
fn name_claims_are_matched_through_the_legacy_scheme_alias() {
    let conn = open_in_memory().expect("in-memory db");
    let location = "autonomi://example/target";

    insert_resource(&conn, RES_A, location);
    // A claim received from a peer may still carry the old scheme, while the
    // resource it points at was indexed under the canonical one.
    repo::names::insert(
        &conn,
        &name_record("n1", "docs", "ant://example/target", None),
    )
    .unwrap();

    assert_eq!(
        repo::names::count_by_target(&conn, location).unwrap(),
        1,
        "the ant:// / autonomi:// alias must not hide a claim from its own resource"
    );

    let cascade = blocks::delete_resource_cascade_with_names(&conn, RES_A).unwrap();
    assert_eq!(cascade.names_removed, 1);
    assert!(
        repo::names::get_by_normalized_name(&conn, "docs")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cascading_an_absent_resource_removes_nothing_and_says_so() {
    let conn = open_in_memory().expect("in-memory db");

    let cascade = blocks::delete_resource_cascade_with_names(&conn, RES_A).unwrap();
    assert!(
        !cascade.resource_removed,
        "an id we never held must not be reported as removed"
    );
    assert_eq!(cascade.names_removed, 0);
}

#[test]
fn a_cached_name_record_round_trips_its_transport_origin() {
    let conn = open_in_memory().expect("in-memory db");

    let record = name_record(
        "nr-1",
        "svc",
        "autonomi://example/t",
        Some("autonomi://example/t"),
    );
    repo::names::insert_with_origin(&conn, &record, Some(AGENT_B)).expect("insert with origin");

    let stored = repo::names::get_by_normalized_name(&conn, "svc").expect("get");
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].origin_agent_id.as_deref(),
        Some(AGENT_B),
        "the authenticated origin must survive the round trip"
    );
    // The self-reported id is a separate column and is left as the peer wrote it.
    assert_eq!(stored[0].by_agent_id, AGENT_A);
}

#[test]
fn a_locally_authored_name_record_has_no_origin() {
    let conn = open_in_memory().expect("in-memory db");

    // `av name` goes through the plain insert, which must leave the origin NULL
    // so agent blocks cannot match it.
    let record = name_record("nr-mine", "mine", "autonomi://example/m", None);
    repo::names::insert(&conn, &record).expect("insert");

    let stored = repo::names::get_by_normalized_name(&conn, "mine").expect("get");
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].origin_agent_id, None,
        "a local record must not be attributed to a peer"
    );
}

#[test]
fn reinserting_a_record_replaces_its_origin() {
    let conn = open_in_memory().expect("in-memory db");

    // `INSERT OR REPLACE` is used for the cache, so a record that first arrives
    // from one peer and later from another must end up attributed to the later
    // one, not keep the first origin and quietly survive that peer's block.
    let record = name_record("nr-r", "moved", "autonomi://example/m", None);
    repo::names::insert_with_origin(&conn, &record, Some(AGENT_A)).expect("first insert");
    repo::names::insert_with_origin(&conn, &record, Some(AGENT_B)).expect("second insert");

    let stored = repo::names::get_by_normalized_name(&conn, "moved").expect("get");
    assert_eq!(
        stored.len(),
        1,
        "the record id is unique, so this is one row"
    );
    assert_eq!(
        stored[0].origin_agent_id.as_deref(),
        Some(AGENT_B),
        "the most recent authenticated origin wins"
    );
}
