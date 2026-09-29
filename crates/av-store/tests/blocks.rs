use av_store::repo::blocks::{self, BlockKind};
use av_store::{open, open_in_memory, repo};

const AGENT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const AGENT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const RES_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";

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
fn counts_by_kind_reports_each_kind() {
    let conn = open_in_memory().expect("in-memory db");
    blocks::add(&conn, BlockKind::Agent, AGENT_A, None, now(), "cli").unwrap();
    blocks::add(&conn, BlockKind::Agent, AGENT_B, None, now(), "cli").unwrap();
    blocks::add(&conn, BlockKind::Resource, RES_A, None, now(), "cli").unwrap();

    let counts = blocks::counts_by_kind(&conn).unwrap();
    let agent_count = counts
        .iter()
        .find(|(k, _)| *k == BlockKind::Agent)
        .map(|(_, n)| *n)
        .unwrap();
    let resource_count = counts
        .iter()
        .find(|(k, _)| *k == BlockKind::Resource)
        .map(|(_, n)| *n)
        .unwrap();
    assert_eq!(agent_count, 2);
    assert_eq!(resource_count, 1);
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
