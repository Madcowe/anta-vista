//! Offline coverage for starter seeding.
//!
//! Everything here runs against the **baked** data in `src/starters_data.rs`:
//! no network, no model. What matters is the *ordering* of the checks in
//! `ensure_seeded` — marker, existing row, block, insert — because each step
//! exists for a user-visible reason: a purged starter must stay gone, a row
//! the user indexed first must be left alone, and a block must both hold now
//! and stay liftable later.

use std::path::PathBuf;
use std::sync::Mutex;

use av_cli::starters::{NO_SEED_ENV, STARTERS, ensure_seeded, seeding_disabled};
use av_core::types::ResourceDescriptor;
use av_store::repo::{blocks, resources, starter_seeds};
use rusqlite::{Connection, OptionalExtension, params};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The three baked descriptors, parsed once per assertion site that needs
/// more than a count.
fn parsed_starters() -> Vec<ResourceDescriptor> {
    assert_eq!(STARTERS.len(), 3, "exactly three starters ship");
    STARTERS
        .iter()
        .map(|s| serde_json::from_str(s.resource_json).expect("baked descriptor must parse"))
        .collect()
}

fn lucky() -> ResourceDescriptor {
    parsed_starters()
        .into_iter()
        .find(|r| r.location_scheme.as_deref() == Some("autonomi"))
        .expect("one starter is an Autonomi record")
}

fn temp_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = dir.path().join("seed.db");
    (dir, db)
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .expect("count")
}

fn description_at(conn: &Connection, location: &str) -> Option<String> {
    conn.query_row(
        "SELECT description_text FROM resources WHERE location = ?1",
        params![location],
        |row| row.get(0),
    )
    .optional()
    .expect("read description")
}

#[test]
fn fresh_database_gets_every_starter() {
    let (_dir, db) = temp_db();
    let report = ensure_seeded(&db).expect("seed");
    assert_eq!((report.seeded, report.adopted, report.blocked), (3, 0, 0));

    let conn = av_store::open(&db).expect("open");
    assert_eq!(count(&conn, "resources"), 3);
    assert_eq!(count(&conn, "embeddings"), 3);
    assert_eq!(count(&conn, "starter_seeds"), 3);

    // The baked vectors sit under the profile searches actually query, or
    // none of the starters could ever be ranked.
    let profile: String = conn
        .query_row("SELECT DISTINCT profile_id FROM embeddings", [], |row| {
            row.get(0)
        })
        .expect("profile");
    assert_eq!(profile, "all-MiniLM-L6-v2:v1");

    // The Autonomi starter carries its tag the same way a user's tags travel:
    // folded into the description text.
    let desc = description_at(&conn, &lucky().location).expect("lucky row");
    assert!(
        desc.contains("tagged as: dog"),
        "tag missing from description: {desc:?}"
    );
}

#[test]
fn baked_vectors_are_valid_minilm_vectors() {
    let mut locations = Vec::new();
    let mut ids = Vec::new();
    for s in STARTERS {
        let r: ResourceDescriptor =
            serde_json::from_str(s.resource_json).expect("descriptor must parse");
        assert_eq!(r.id.len(), 64, "resource ids are sha256 hex");
        assert!(!r.description_text.is_empty(), "empty description");
        assert_eq!(s.vector.len(), 384, "{} is not a MiniLM vector", r.location);
        let norm = s.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-3,
            "{} vector is not unit-norm ({norm})",
            r.location
        );
        locations.push(r.location);
        ids.push(r.id);
    }
    locations.sort();
    ids.sort();
    locations.dedup();
    ids.dedup();
    assert_eq!(locations.len(), 3, "locations must be unique");
    assert_eq!(ids.len(), 3, "resource ids must be unique");
}

#[test]
fn seeding_twice_writes_nothing_more() {
    let (_dir, db) = temp_db();
    let first = ensure_seeded(&db).expect("seed");
    assert_eq!(first.seeded, 3);

    let second = ensure_seeded(&db).expect("seed again");
    assert_eq!(
        (second.seeded, second.adopted, second.blocked),
        (0, 0, 0),
        "markers must make a second pass a no-op"
    );

    let conn = av_store::open(&db).expect("open");
    assert_eq!(count(&conn, "resources"), 3);
    assert_eq!(count(&conn, "embeddings"), 3);
    assert_eq!(count(&conn, "starter_seeds"), 3);
}

#[test]
fn a_purged_starter_stays_purged() {
    let (_dir, db) = temp_db();
    ensure_seeded(&db).expect("seed");

    let victim = lucky();
    {
        let conn = av_store::open(&db).expect("open");
        resources::delete(&conn, &victim.id).expect("purge");
    }

    let report = ensure_seeded(&db).expect("seed again");
    assert_eq!(
        (report.seeded, report.adopted, report.blocked),
        (0, 0, 0),
        "a purged starter must not be re-seeded on the next boot"
    );
    let conn = av_store::open(&db).expect("open");
    assert_eq!(
        description_at(&conn, &victim.location),
        None,
        "the purged starter came back"
    );
    assert!(
        starter_seeds::is_seeded(&conn, &victim.location).expect("marker read"),
        "the marker is what remembers the purge"
    );
}

#[test]
fn a_row_the_user_indexed_first_is_adopted_not_overwritten() {
    let (_dir, db) = temp_db();
    let conn = av_store::open(&db).expect("open");

    let theirs = parsed_starters()
        .into_iter()
        .find(|r| r.location_scheme.as_deref() == Some("https"))
        .expect("an https starter");
    let mut own = theirs.clone();
    own.id = "1".repeat(64);
    own.description_text = "my own copy of the page".to_string();
    resources::insert(&conn, &own).expect("insert user row");

    let report = ensure_seeded(&db).expect("seed");
    assert_eq!(
        (report.seeded, report.adopted, report.blocked),
        (2, 1, 0),
        "the user's row counts as settled, not as a failure"
    );

    let kept: ResourceDescriptor = av_store::repo::resources::get_by_location(&conn, &own.location)
        .expect("read")
        .expect("row still there");
    assert_eq!(kept.id, own.id, "the user's id must survive");
    assert_eq!(
        kept.description_text, "my own copy of the page",
        "seeding must never overwrite the user's description"
    );
    assert!(
        starter_seeds::is_seeded(&conn, &own.location).expect("marker read"),
        "an adopted location must be marked so later boots leave it alone"
    );
    assert_eq!(count(&conn, "resources"), 3);
}

#[test]
fn a_blocked_starter_is_skipped_until_unblocked() {
    let (_dir, db) = temp_db();
    let victim = lucky();
    {
        let conn = av_store::open(&db).expect("open");
        blocks::add(
            &conn,
            blocks::BlockKind::Resource,
            &victim.id,
            None,
            1_700_000_000,
            "test",
        )
        .expect("block");
    }

    let report = ensure_seeded(&db).expect("seed");
    assert_eq!(
        (report.seeded, report.adopted, report.blocked),
        (2, 0, 1),
        "a blocked starter must not be written over the block"
    );
    {
        let conn = av_store::open(&db).expect("open");
        assert!(
            !starter_seeds::is_seeded(&conn, &victim.location).expect("marker read"),
            "a blocked skip must stay unmarked, or unblocking could never admit it"
        );
        assert_eq!(description_at(&conn, &victim.location), None);

        // The reversibility the missing marker exists for.
        blocks::remove(&conn, blocks::BlockKind::Resource, &victim.id).expect("unblock");
    }

    let report = ensure_seeded(&db).expect("seed again");
    assert_eq!(
        (report.seeded, report.adopted, report.blocked),
        (1, 0, 0),
        "after an unblock the next boot must land the starter"
    );
    let conn = av_store::open(&db).expect("open");
    assert!(description_at(&conn, &victim.location).is_some());
    assert_eq!(count(&conn, "resources"), 3);
}

#[test]
fn the_seed_gate_accepts_the_autostart_spellings() {
    // Only this file reads the gate, and only this test writes it.
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::remove_var(NO_SEED_ENV) };
    assert!(!seeding_disabled(), "unset means seeded");

    for on in ["1", "true", "yes"] {
        unsafe { std::env::set_var(NO_SEED_ENV, on) };
        assert!(seeding_disabled(), "{on} must disable seeding");
    }
    for off in ["0", "", "no", "false"] {
        unsafe { std::env::set_var(NO_SEED_ENV, off) };
        assert!(!seeding_disabled(), "{off} must leave seeding on");
    }
    unsafe { std::env::remove_var(NO_SEED_ENV) };
}
