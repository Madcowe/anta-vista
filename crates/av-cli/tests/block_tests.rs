//! Local block policy, exercised through the real command handlers.
//!
//! These tests drive `cmd::block::run` / `run_unblock` in-process against a
//! throwaway database, the same way `cli_tests.rs` drives the other commands.
//! They cover the parts of the feature that are about *this node's* behaviour:
//! what a block does to local data, what it reports, and that the egress paths
//! refuse a blocked resource.
//!
//! Network fan-out (excluding a blocked agent from candidate lists, dropping
//! its responses) needs a live x0x daemon and is covered by the daemon tests.

use av_cli::cmd::block;
use av_cli::startup::StartupState;
use av_core::config::AvConfig;
use av_core::types::{
    EmbeddingProfile, EmbeddingRecord, FeedbackEvent, FeedbackKind, ResourceDescriptor,
    ResourceKind,
};
use av_embed::mock::MockEmbeddingProvider;
use av_embed::provider::EmbeddingProvider as _;
use av_store::repo::blocks::{self, BlockKind};
use std::sync::Mutex;
use tempfile::tempdir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Take the env-var lock, tolerating poisoning.
///
/// These tests all mutate `ANTA_VISTA_DB_PATH`, so they cannot run in parallel.
/// A plain `.lock().unwrap()` makes one panicking test poison the lock and turn
/// a single real failure into a dozen `PoisonError`s, which buries the actual
/// cause. Poisoning here only ever means "a sibling test panicked"; the
/// remaining tests still have to run to show their own results.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn cli() -> av_cli::Cli {
    av_cli::Cli {
        non_interactive: true,
        config: None,
        timeout: 1000,
        stream: false,
        verbose: 0,
        command: None,
    }
}

/// A `StartupState` with no x0x daemon, i.e. the "network is down" case. A
/// block must work regardless, which is why these commands are in
/// `is_offline_command`.
fn offline_state() -> StartupState {
    StartupState {
        config: AvConfig::default(),
        config_path: None,
        x0x_config: None,
        antd_running: false,
        minilm_loaded: false,
        listener_running: false,
    }
}

/// Set up a database holding one fully-populated resource: the row itself, an
/// embedding, a feedback event and a relevance judgment. Populating all three
/// dependants is the point — a block that appears to clean up but quietly
/// leaves feedback behind is worse than one that says what it did.
fn setup_with_populated_resource(resource_id: &str) -> (tempfile::TempDir, rusqlite::Connection) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("blocks.db");
    // SAFETY: tests are Mutex-guarded, so env var access is serialized.
    unsafe { std::env::set_var("ANTA_VISTA_DB_PATH", &db_path) };

    let conn = av_store::open(&db_path).unwrap();

    av_store::repo::resources::insert(
        &conn,
        &ResourceDescriptor {
            id: resource_id.to_string(),
            kind: ResourceKind::Text,
            location: "autonomi://deadbeef".to_string(),
            location_scheme: Some("autonomi".to_string()),
            location_canonical: Some("autonomi://deadbeef".to_string()),
            mime_type: "text/plain".to_string(),
            filename: None,
            metadata_json: serde_json::json!({}),
            description_text: "A document about foxes.".to_string(),
            created_at: 1_700_000_000,
        },
    )
    .unwrap();

    let profile = EmbeddingProfile {
        model_id: "mock".to_string(),
        model_version: "1".to_string(),
        dim: 8,
        normalized: true,
        preproc_version: "1".to_string(),
    };
    let pid = av_embed::provider::profile_id(&profile);
    av_store::repo::embeddings::insert_profile(&conn, &pid, &profile).unwrap();
    av_store::repo::embeddings::insert(
        &conn,
        &EmbeddingRecord {
            resource_id: resource_id.to_string(),
            profile_id: pid,
            vector: vec![0.1; 8],
            l2_norm: 1.0,
            created_at: 1_700_000_000,
        },
    )
    .unwrap();

    av_store::repo::feedback::insert(
        &conn,
        &FeedbackEvent {
            schema_version: 1,
            feedback_id: "fb-1".to_string(),
            query_text: "fox".to_string(),
            resource_id: resource_id.to_string(),
            by_agent_id: "agent-abc".to_string(),
            kind: FeedbackKind::Useful,
            timestamp: 1_700_000_000,
            signature: vec![],
        },
    )
    .unwrap();

    av_store::repo::relevance::upsert(&conn, "fox", resource_id, 1.0).unwrap();

    (dir, conn)
}

fn count_dependants(conn: &rusqlite::Connection, resource_id: &str) -> i64 {
    conn.query_row(
        "SELECT
            (SELECT COUNT(*) FROM embeddings          WHERE resource_id = ?1)
          + (SELECT COUNT(*) FROM feedback_events     WHERE resource_id = ?1)
          + (SELECT COUNT(*) FROM relevance_judgments WHERE resource_id = ?1)",
        rusqlite::params![resource_id],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn blocking_a_resource_deletes_it_and_its_dependants_by_default() {
    let _guard = env_lock();
    let (dir, conn) = setup_with_populated_resource("res-to-block");
    assert_eq!(
        count_dependants(&conn, "res-to-block"),
        3,
        "control: 3 dependants"
    );

    let outcome = block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Resource,
        "res-to-block",
        Some("spam"),
        /* keep_local = */ false,
        /* x0x = */ false,
        "test",
    )
    .unwrap();

    assert!(
        outcome.blocked,
        "the first block of a subject is a real change"
    );
    assert!(
        outcome.removed_local,
        "the local copy should have been deleted"
    );
    assert!(!outcome.kept_local);
    // One embedding + one feedback event + one relevance judgment. Reported
    // rather than assumed, because silently dropping feedback would corrupt
    // the operator's own rating history without them being told.
    assert_eq!(outcome.cascaded, 3);

    assert!(
        av_store::repo::resources::get(&conn, "res-to-block")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        count_dependants(&conn, "res-to-block"),
        0,
        "no orphans left behind"
    );
    assert!(blocks::is_blocked(&conn, BlockKind::Resource, "res-to-block").unwrap());

    drop(dir);
}

#[test]
fn keep_local_hides_the_resource_but_keeps_every_row() {
    let _guard = env_lock();
    let (_dir, conn) = setup_with_populated_resource("res-keep");

    let outcome = block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Resource,
        "res-keep",
        None,
        /* keep_local = */ true,
        false,
        "test",
    )
    .unwrap();

    assert!(outcome.blocked);
    assert!(!outcome.removed_local, "--keep-local must not delete");
    assert!(outcome.kept_local);
    assert_eq!(
        outcome.cascaded, 0,
        "nothing was deleted, so nothing cascaded"
    );

    assert!(
        av_store::repo::resources::get(&conn, "res-keep")
            .unwrap()
            .is_some(),
        "--keep-local is the escape hatch for keeping local data"
    );
    assert_eq!(count_dependants(&conn, "res-keep"), 3);
    assert!(blocks::is_blocked(&conn, BlockKind::Resource, "res-keep").unwrap());

    // And the block still hides it from search, which is the whole point:
    // retained data must not reappear in results.
    let provider = MockEmbeddingProvider::new();
    let profile = provider.profile().clone();
    let pid = av_embed::provider::profile_id(&profile);
    let hits = av_index::search::search_top_k(
        &conn,
        &[0.1; 8],
        &pid,
        5,
        &av_index::QueryFilter::default(),
        "fox",
    )
    .unwrap();
    assert!(
        hits.iter().all(|h| h.resource.id != "res-keep"),
        "a --keep-local block must still hide the resource from search"
    );
}

#[test]
fn unblocking_does_not_claim_to_restore_deleted_data() {
    let _guard = env_lock();
    let (_dir, conn) = setup_with_populated_resource("res-gone");

    block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Resource,
        "res-gone",
        None,
        false,
        false,
        "test",
    )
    .unwrap();

    let out = block::unblock_subject(
        &conn,
        &offline_state(),
        BlockKind::Resource,
        Some("res-gone"),
        false,
        false,
    )
    .unwrap();

    assert!(out.unblocked);
    assert!(!blocks::is_blocked(&conn, BlockKind::Resource, "res-gone").unwrap());
    // This is the important half: unblocking lifts the policy, it does not
    // undo the delete. The resource has to be re-indexed from its URI.
    assert!(
        av_store::repo::resources::get(&conn, "res-gone")
            .unwrap()
            .is_none(),
        "unblock must not resurrect a deleted resource"
    );
}

#[test]
fn unblocking_a_resource_that_was_never_blocked_reports_nothing_changed() {
    let _guard = env_lock();
    let (_dir, conn) = setup_with_populated_resource("res-untouched");

    let out = block::unblock_subject(
        &conn,
        &offline_state(),
        BlockKind::Resource,
        Some("res-untouched"),
        false,
        false,
    )
    .unwrap();

    assert!(
        !out.unblocked,
        "the caller must be able to say 'was not blocked'"
    );
    assert_eq!(out.removed, 0);
}

#[test]
fn a_name_block_hides_the_record_without_deleting_it() {
    let _guard = env_lock();
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("blocks.db");
    unsafe { std::env::set_var("ANTA_VISTA_DB_PATH", &db_path) };
    let conn = av_store::open(&db_path).unwrap();

    let record = av_core::types::NameRecord {
        schema_version: 1,
        record_id: "nr-1".to_string(),
        normalized_name: av_core::types::normalize_name("Contested"),
        original_name: "Contested".to_string(),
        record_type: av_core::types::NameRecordType::Uri,
        target: "autonomi://aa".to_string(),
        target_scheme: Some("autonomi".to_string()),
        target_canonical: Some("autonomi://aa".to_string()),
        ttl_secs: 3600,
        by_agent_id: "agent-abc".to_string(),
        timestamp: 1_700_000_000,
        signature: vec![],
    };
    av_store::repo::names::insert(&conn, &record).unwrap();

    let outcome = block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Name,
        "nr-1",
        None,
        false,
        false,
        "test",
    )
    .unwrap();

    assert!(outcome.blocked);
    assert!(
        !outcome.removed_local,
        "a name block hides; it does not delete"
    );
    assert!(
        av_store::repo::names::get_by_normalized_name(
            &conn,
            &av_core::types::normalize_name("Contested")
        )
        .unwrap()
        .iter()
        .any(|r| r.record_id == "nr-1"),
        "the record must survive so unblocking can restore it"
    );
}

#[test]
fn an_agent_id_that_cannot_be_an_agent_id_is_rejected() {
    let _guard = env_lock();
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("blocks.db");
    unsafe { std::env::set_var("ANTA_VISTA_DB_PATH", &db_path) };
    let conn = av_store::open(&db_path).unwrap();

    // A mistyped id stored as a block would silently never match anything,
    // which reads as "the block is working" while doing nothing at all.
    for bad in ["nothex", "abc", &"z".repeat(64), &"a".repeat(63)] {
        let err = block::block_subject(
            &conn,
            &offline_state(),
            BlockKind::Agent,
            bad,
            None,
            false,
            false,
            "test",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("not a valid agent id"),
            "'{bad}' should have been rejected, got: {err}"
        );
    }
    assert_eq!(
        blocks::counts_by_kind(&conn).unwrap().len(),
        0,
        "nothing was stored"
    );
}

#[test]
fn a_pasted_name_prefix_is_accepted_and_normalized() {
    let _guard = env_lock();
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("blocks.db");
    unsafe { std::env::set_var("ANTA_VISTA_DB_PATH", &db_path) };
    let conn = av_store::open(&db_path).unwrap();

    // Search results label a name record's id as `name:<id>`. Pasting one of
    // those straight into `av block --name` should block that record, not
    // create a block on a literal id that matches nothing.
    let outcome = block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Name,
        "name:nr-9",
        None,
        false,
        false,
        "test",
    )
    .unwrap();

    assert_eq!(outcome.subject_id, "nr-9");
    assert!(blocks::is_blocked(&conn, BlockKind::Name, "nr-9").unwrap());
}

#[test]
fn blocking_works_with_the_network_down() {
    let _guard = env_lock();
    let (_dir, conn) = setup_with_populated_resource("res-offline");

    // `offline_state()` has `x0x_config: None`. Blocking a peer is exactly the
    // operation you need when the peer is misbehaving, and that is not a moment
    // to be told the daemon is down.
    let outcome = block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Resource,
        "res-offline",
        Some("done while offline"),
        false,
        false,
        "test",
    )
    .unwrap();
    assert!(outcome.blocked);
    assert!(!outcome.x0x_requested);
    assert!(
        outcome.x0x_applied.is_none(),
        "no daemon means no transport block"
    );
}

#[test]
fn an_x0x_escalation_is_reported_as_not_applied_when_the_daemon_is_down() {
    let _guard = env_lock();
    let (_dir, conn) = setup_with_populated_resource("res-x0x");
    let agent = "a".repeat(64);

    let outcome = block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Agent,
        &agent,
        None,
        false,
        /* x0x = */ true,
        "test",
    )
    .unwrap();

    assert!(
        outcome.blocked,
        "the local block lands regardless of the daemon"
    );
    assert!(outcome.x0x_requested);
    // No daemon, so nothing was escalated. The caller must be able to tell the
    // difference between "asked for and done" and "asked for and not done".
    assert_eq!(outcome.x0x_applied, None);
}

#[test]
fn the_three_kinds_are_independent_policies() {
    let _guard = env_lock();
    let (_dir, conn) = setup_with_populated_resource("shared-id");

    // Block a resource, then an agent, then a name record, all with the same
    // subject string. None of them may suppress the others.
    let state = offline_state();
    block::block_subject(
        &conn,
        &state,
        BlockKind::Resource,
        "shared-id",
        None,
        true,
        false,
        "t",
    )
    .unwrap();
    block::block_subject(
        &conn,
        &state,
        BlockKind::Name,
        "shared-id",
        None,
        false,
        false,
        "t",
    )
    .unwrap();

    let blocked = blocks::blocked_ids(&conn, BlockKind::Resource).unwrap();
    assert!(blocked.contains("shared-id"));
    let names = blocks::blocked_ids(&conn, BlockKind::Name).unwrap();
    assert!(names.contains("shared-id"));

    // Clearing one kind leaves the other alone.
    blocks::clear(&conn, Some(BlockKind::Resource)).unwrap();
    assert!(!blocks::is_blocked(&conn, BlockKind::Resource, "shared-id").unwrap());
    assert!(
        blocks::is_blocked(&conn, BlockKind::Name, "shared-id").unwrap(),
        "clearing resource blocks must not clear name blocks"
    );
}

#[test]
fn list_reports_every_kind_with_a_count() {
    let _guard = env_lock();
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("blocks.db");
    unsafe { std::env::set_var("ANTA_VISTA_DB_PATH", &db_path) };
    let conn = av_store::open(&db_path).unwrap();
    let state = offline_state();

    block::block_subject(
        &conn,
        &state,
        BlockKind::Resource,
        "r1",
        Some("spam"),
        false,
        false,
        "t",
    )
    .unwrap();
    block::block_subject(
        &conn,
        &state,
        BlockKind::Name,
        "n1",
        None,
        false,
        false,
        "t",
    )
    .unwrap();
    block::block_subject(
        &conn,
        &state,
        BlockKind::Agent,
        &"b".repeat(64),
        None,
        false,
        false,
        "t",
    )
    .unwrap();

    let entries = block::list_blocks(&conn).unwrap();
    assert_eq!(entries.len(), 3);
    assert!(
        entries
            .iter()
            .any(|e| e.subject_id == "r1" && e.reason.as_deref() == Some("spam"))
    );

    let counts = block::counts(&conn).unwrap();
    assert_eq!(counts.iter().map(|(_, n)| *n).sum::<i64>(), 3);
}

#[test]
fn a_blocked_resource_cannot_be_reintroduced_by_propagate() {
    let _guard = env_lock();
    let (_dir, conn) = setup_with_populated_resource("res-reintro");
    let provider = MockEmbeddingProvider::new();
    let state = offline_state();

    block::block_subject(
        &conn,
        &state,
        BlockKind::Resource,
        "res-reintro",
        None,
        /* keep_local = */ true,
        false,
        "test",
    )
    .unwrap();

    // `propagate_resource` is the shared entry point behind `av propagate`, the
    // interactive search prompt and the web rating handler. Re-inserting here
    // would quietly undo a block, which is the one way a block could be lost.
    let err = av_cli::cmd::propagate::propagate_resource(
        &conn,
        &provider,
        "res-reintro",
        "autonomi://deadbeef",
        "A description that would re-embed this resource.",
        "text/plain",
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("blocked"),
        "the error should say it is blocked, got: {msg}"
    );
    assert!(
        msg.contains("av unblock --resource res-reintro"),
        "the error should name the way out, got: {msg}"
    );
}

#[test]
fn propagate_still_works_for_an_unblocked_resource() {
    let _guard = env_lock();
    let (_dir, conn) = setup_with_populated_resource("res-ok");
    let provider = MockEmbeddingProvider::new();

    av_cli::cmd::propagate::propagate_resource(
        &conn,
        &provider,
        "res-fresh",
        "autonomi://beef",
        "A brand new resource.",
        "text/plain",
    )
    .expect("propagating an unblocked resource must still work");

    assert!(
        av_store::repo::resources::get(&conn, "res-fresh")
            .unwrap()
            .is_some(),
        "the guard must not block ordinary propagation"
    );
}

#[test]
fn the_block_and_unblock_commands_run_offline_end_to_end() {
    let _guard = env_lock();
    let (_dir, conn) = setup_with_populated_resource("res-e2e");

    // Drive the actual command handlers, not just the library functions, so the
    // argument plumbing and JSON reporting are covered too.
    block::run(
        cli(),
        offline_state(),
        Some("res-e2e".to_string()),
        None,
        None,
        Some("spam".to_string()),
        false,
        false,
        true, // --list
    )
    .unwrap();
    assert!(
        av_store::repo::resources::get(&conn, "res-e2e")
            .unwrap()
            .is_some()
    );

    block::run(
        cli(),
        offline_state(),
        Some("res-e2e".to_string()),
        None,
        None,
        Some("spam".to_string()),
        false,
        false,
        false,
    )
    .unwrap();
    assert!(
        av_store::repo::resources::get(&conn, "res-e2e")
            .unwrap()
            .is_none()
    );

    block::run_unblock(
        cli(),
        offline_state(),
        Some("res-e2e".to_string()),
        None,
        None,
        false,
        false,
        true, // no_confirm
    )
    .unwrap();
    assert!(!blocks::is_blocked(&conn, BlockKind::Resource, "res-e2e").unwrap());
}

// --- Getting a usable id out of the output ----------------------------------
//
// A block is keyed by exact match, so the id the user copies is the whole
// feature. These cover the case where a short id was pasted from an
// abbreviated display: it must not become a block that silently matches
// nothing.

#[test]
fn an_abbreviated_id_of_a_local_resource_is_refused_with_the_full_id() {
    let _guard = env_lock();
    let (dir, conn) = setup_with_populated_resource(
        "aaaabbbbccccddddeeeeffff00001111222233334444555566667777888899990",
    );

    let err = block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Resource,
        "aaaabbbbcccc",
        None,
        false,
        false,
        "test",
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("abbreviation"),
        "should explain it is abbreviated, got: {msg}"
    );
    assert!(
        msg.contains("aaaabbbbccccddddeeeeffff00001111222233334444555566667777888899990"),
        "should offer the full id, got: {msg}"
    );
    // Crucially, nothing was stored: a dead block is worse than an error.
    assert!(
        !blocks::is_blocked(
            &conn,
            BlockKind::Resource,
            "aaaabbbbccccddddeeeeffff00001111222233334444555566667777888899990"
        )
        .unwrap(),
        "the full id must not have been blocked by an abbreviated request"
    );
    assert_eq!(blocks::counts_by_kind(&conn).unwrap().len(), 0);

    drop(dir);
}

#[test]
fn the_full_id_is_not_mistaken_for_an_abbreviation_of_itself() {
    let _guard = env_lock();
    let (_dir, conn) = setup_with_populated_resource(
        "aaaabbbbccccddddeeeeffff00001111222233334444555566667777888899990",
    );

    // The prefix lookup has to exclude the subject's own row, or a full id
    // looks like a prefix of itself and nothing can ever be blocked.
    let outcome = block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Resource,
        "aaaabbbbccccddddeeeeffff00001111222233334444555566667777888899990",
        None,
        /* keep_local = */ true,
        false,
        "test",
    )
    .expect("the full id must be accepted");
    assert!(outcome.blocked);
}

#[test]
fn an_abbreviated_id_of_a_local_name_record_is_refused() {
    let _guard = env_lock();
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("blocks.db");
    unsafe { std::env::set_var("ANTA_VISTA_DB_PATH", &db_path) };
    let conn = av_store::open(&db_path).unwrap();

    av_store::repo::names::insert(
        &conn,
        &av_core::types::NameRecord {
            schema_version: 1,
            record_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            normalized_name: av_core::types::normalize_name("Truncated"),
            original_name: "Truncated".to_string(),
            record_type: av_core::types::NameRecordType::Uri,
            target: "autonomi://aa".to_string(),
            target_scheme: Some("autonomi".to_string()),
            target_canonical: Some("autonomi://aa".to_string()),
            ttl_secs: 3600,
            by_agent_id: "agent-abc".to_string(),
            timestamp: 1_700_000_000,
            signature: vec![],
        },
    )
    .unwrap();

    let err = block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Name,
        "550e8400",
        None,
        false,
        false,
        "test",
    )
    .unwrap_err();
    assert!(err.to_string().contains("abbreviation"), "got: {err}");
    assert!(
        err.to_string()
            .contains("550e8400-e29b-41d4-a716-446655440000")
    );
    assert_eq!(blocks::counts_by_kind(&conn).unwrap().len(), 0);
}

#[test]
fn an_unknown_short_id_is_still_allowed_but_warns() {
    let _guard = env_lock();
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("blocks.db");
    unsafe { std::env::set_var("ANTA_VISTA_DB_PATH", &db_path) };
    let conn = av_store::open(&db_path).unwrap();

    // A network result's resource is legitimately not held locally, so
    // refusing to block it would be a real limitation. The block lands, and the
    // response says it may not match.
    let outcome = block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Resource,
        "abc123",
        None,
        false,
        false,
        "test",
    )
    .expect("an unknown id must remain blockable");
    assert!(outcome.blocked);
    assert!(blocks::is_blocked(&conn, BlockKind::Resource, "abc123").unwrap());
}

#[test]
fn a_wildcard_in_an_id_does_not_match_other_subjects() {
    let _guard = env_lock();
    let (dir, conn) = setup_with_populated_resource(
        "aaaabbbbccccddddeeeeffff00001111222233334444555566667777888899990",
    );

    // `%` and `_` are `LIKE` wildcards. An id is user-supplied, so a `%` in it
    // must not turn the prefix lookup into a pattern that matches everything.
    let outcome = block::block_subject(
        &conn,
        &offline_state(),
        BlockKind::Resource,
        "%",
        None,
        false,
        false,
        "test",
    )
    .expect("a wildcard id must not be resolved against other rows");
    assert!(outcome.blocked);
    assert_eq!(outcome.subject_id, "%");

    drop(dir);
}

#[test]
fn a_dropped_blocks_table_fails_closed_rather_than_serving_everything() {
    let _guard = env_lock();
    let (_dir, conn) = setup_with_populated_resource(
        "aaaabbbbccccddddeeeeffff00001111222233334444555566667777888899990",
    );
    assert!(blocks::counts_by_kind(&conn).is_ok(), "precondition");

    // Simulate the block list becoming unreadable the way a real failure would:
    // the table is gone. Every policy read must refuse rather than treat the
    // error as "nothing is blocked", which is what would let a suppressed
    // resource leave the node.
    conn.execute("DROP TABLE blocks", []).unwrap();

    // Must use the profile the fixture actually embedded under: with an
    // unknown profile `search_top_k` returns early with no candidates and never
    // reaches the block read, so the test would pass for the wrong reason.
    let profile = EmbeddingProfile {
        model_id: "mock".to_string(),
        model_version: "1".to_string(),
        dim: 8,
        normalized: true,
        preproc_version: "1".to_string(),
    };
    let pid = av_embed::provider::profile_id(&profile);
    av_store::repo::embeddings::insert_profile(&conn, &pid, &profile).unwrap();
    av_store::repo::embeddings::insert(
        &conn,
        &EmbeddingRecord {
            resource_id: "aaaabbbbccccddddeeeeffff00001111222233334444555566667777888899990"
                .to_string(),
            profile_id: pid.clone(),
            vector: vec![0.1; 8],
            l2_norm: 1.0,
            created_at: 1_700_000_000,
        },
    )
    .unwrap();

    let err = av_index::search::search_top_k(
        &conn,
        &[0.1; 8],
        &pid,
        10,
        &av_index::filter::QueryFilter::default(),
        "anything",
    );
    assert!(
        err.is_err(),
        "search must fail closed when the block list is unreadable, got {err:?}"
    );
    // A name record has to exist: `lookup_name` returns `Ok(vec![])` early when
    // there is nothing to serve, which is harmless but would make this assert
    // pass for the wrong reason.
    av_store::repo::names::insert(
        &conn,
        &av_core::types::NameRecord {
            schema_version: 1,
            record_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            normalized_name: av_core::types::normalize_name("failclosed"),
            original_name: "failclosed".to_string(),
            record_type: av_core::types::NameRecordType::Uri,
            target: "autonomi://aa".to_string(),
            target_scheme: Some("autonomi".to_string()),
            target_canonical: Some("autonomi://aa".to_string()),
            ttl_secs: 3600,
            by_agent_id: "agent-abc".to_string(),
            timestamp: 1_700_000_000,
            signature: vec![],
        },
    )
    .unwrap();
    let err = av_index::naming::lookup_name(
        &conn,
        "failclosed",
        &av_index::filter::SchemeFilter::default(),
        0,
    );
    assert!(
        err.is_err(),
        "name lookup must fail closed when the block list is unreadable, got {err:?}"
    );
}
