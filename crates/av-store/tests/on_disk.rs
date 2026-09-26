use av_core::types::{ResourceDescriptor, ResourceKind};
use av_store::{open, repo};
use tempfile::tempdir;

#[test]
fn test_open_on_disk_creates_file() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");

    assert!(!db_path.exists(), "DB should not exist before open");
    let conn = open(&db_path).expect("open DB");
    assert!(db_path.exists(), "DB file should be created by open()");

    // Verify WAL mode
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .expect("pragma");
    assert_eq!(mode, "wal", "WAL mode should be enabled");

    // Verify FK enabled
    let fk: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .expect("pragma");
    assert_eq!(fk, 1, "foreign_keys should be ON");
}

#[test]
fn test_migration_idempotent_on_disk() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("idempotent.db");

    // Open twice — migrations must not fail or duplicate on second open
    {
        let _conn1 = open(&db_path).expect("first open");
    }
    {
        let _conn2 = open(&db_path).expect("second open should succeed");
    }
}

#[test]
fn test_concurrent_opens_of_a_fresh_database() {
    // The web server opens a connection per request, so a burst of requests
    // against a brand-new database runs `migrate` on several threads at once.
    // They all see the migrations as pending; only the bookkeeping insert
    // needs to tolerate the race.
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("concurrent.db");

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let db_path = db_path.clone();
            std::thread::spawn(move || open(&db_path).map(|_| ()))
        })
        .collect();

    for h in handles {
        h.join()
            .expect("thread should not panic")
            .expect("open should succeed");
    }

    // And the result is a fully migrated database, not a half-applied one.
    let conn = open(&db_path).expect("final open");
    let applied: i64 = conn
        .query_row("SELECT COUNT(*) FROM applied_migrations", [], |r| r.get(0))
        .expect("count migrations");
    assert_eq!(
        applied, 4,
        "every migration should be recorded exactly once"
    );
}

#[test]
fn test_resource_persists_across_connections() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("persist.db");

    // Write
    {
        let conn = open(&db_path).expect("open");
        let resource = ResourceDescriptor {
            id: "persist-001".into(),
            kind: ResourceKind::Text,
            location: "https://example.com/test.txt".into(),
            location_scheme: Some("https".into()),
            location_canonical: None,
            mime_type: "text/plain".into(),
            filename: Some("test.txt".into()),
            metadata_json: serde_json::json!({}),
            description_text: "a test text document".into(),
            created_at: 1_700_000_000,
        };
        repo::resources::insert(&conn, &resource).expect("insert");
    }

    // Read in a new connection
    {
        let conn = open(&db_path).expect("reopen");
        let found = repo::resources::get(&conn, "persist-001").expect("get");
        assert!(
            found.is_some(),
            "resource should persist across connections"
        );
        assert_eq!(found.unwrap().description_text, "a test text document");
    }
}

#[test]
fn test_path_with_nested_directories() {
    let dir = tempdir().expect("tempdir");
    // open() must create parent directories
    let db_path = dir.path().join("a").join("b").join("c").join("nested.db");
    let conn = open(&db_path).expect("open with nested path");
    // Should work fine
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM applied_migrations", [], |r| r.get(0))
        .expect("query");
    assert!(count > 0, "migrations should have run");
}
