use av_core::types::{NameRecord, NameRecordType, normalize_name};
use av_embed::mock::MockEmbeddingProvider;
use av_embed::provider::{EmbeddingProvider, profile_id};
use av_index::{LocalIndex, MimeFilter, QueryFilter, SchemeFilter};
use av_ingest::ingest_bytes;
use av_store::{open_in_memory, repo};

// Minimal JPEG magic bytes
const JPEG_BYTES: &[u8] = &[
    0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01,
    0x00, 0x01, 0x00, 0x00, 0xFF, 0xD9,
];
const TEXT_BYTES: &[u8] = b"This is a document about Rust programming language features.";

fn setup() -> (rusqlite::Connection, MockEmbeddingProvider) {
    let conn = open_in_memory().expect("open db");
    let provider = MockEmbeddingProvider::new();
    (conn, provider)
}

fn store_resource(
    conn: &rusqlite::Connection,
    provider: &MockEmbeddingProvider,
    bytes: &[u8],
    filename: &str,
    location: &str,
) {
    let resource = ingest_bytes(bytes, Some(filename), location).expect("ingest");
    repo::resources::insert(conn, &resource).expect("insert resource");

    // Insert profile
    let profile = provider.profile().clone();
    let pid = profile_id(&profile);
    repo::embeddings::insert_profile(conn, &pid, &profile).expect("insert profile");

    // Embed and store
    let record = provider
        .embed_resource(&resource.description_text, &resource.id)
        .expect("embed");
    repo::embeddings::insert(conn, &record).expect("insert embedding");
}

#[test]
fn test_search_returns_results() {
    let (conn, provider) = setup();

    store_resource(
        &conn,
        &provider,
        JPEG_BYTES,
        "fish.jpg",
        "https://example.com/fish.jpg",
    );
    store_resource(
        &conn,
        &provider,
        TEXT_BYTES,
        "rustdoc.txt",
        "https://example.com/rustdoc.txt",
    );

    let index = LocalIndex::new(&conn, &provider);
    let filter = QueryFilter::default();
    let results = index.search("image file", 5, &filter).expect("search");

    assert!(!results.is_empty(), "should return at least one result");
    for r in &results {
        assert!(
            r.score >= -1.0 && r.score <= 1.0,
            "cosine score must be in [-1, 1]"
        );
    }
}

#[test]
fn test_search_scheme_filter() {
    let (conn, provider) = setup();

    store_resource(
        &conn,
        &provider,
        JPEG_BYTES,
        "fish.jpg",
        "https://example.com/fish.jpg",
    );
    store_resource(
        &conn,
        &provider,
        TEXT_BYTES,
        "antfile.txt",
        "ant://deadbeef01234567890123456789012345678901234567890123456789012345",
    );

    let index = LocalIndex::new(&conn, &provider);

    // Filter to https only
    let mut filter = QueryFilter::default();
    filter.scheme = SchemeFilter::new(["https"]);
    let results = index.search("file", 10, &filter).expect("search");

    for r in &results {
        assert_eq!(
            r.resource.location_scheme.as_deref(),
            Some("https"),
            "scheme filter should only return https resources"
        );
    }
}

#[test]
fn test_search_top_k_limit() {
    let (conn, provider) = setup();

    // Insert 5 resources
    for i in 0..5 {
        let text = format!("document number {} about various topics", i);
        let resource = ingest_bytes(
            text.as_bytes(),
            Some(&format!("doc{}.txt", i)),
            &format!("https://example.com/doc{}.txt", i),
        )
        .expect("ingest");
        repo::resources::insert(&conn, &resource).expect("insert");
        let profile = provider.profile().clone();
        let pid = profile_id(&profile);
        repo::embeddings::insert_profile(&conn, &pid, &profile).ok();
        let record = provider
            .embed_resource(&resource.description_text, &resource.id)
            .expect("embed");
        repo::embeddings::insert(&conn, &record).expect("insert emb");
    }

    let index = LocalIndex::new(&conn, &provider);
    let results = index
        .search("document", 3, &QueryFilter::default())
        .expect("search");
    assert!(results.len() <= 3, "top-k=3 must return at most 3 results");
}

#[test]
fn test_search_empty_index() {
    let (conn, provider) = setup();
    let index = LocalIndex::new(&conn, &provider);
    let results = index
        .search("anything", 10, &QueryFilter::default())
        .expect("search");
    assert!(results.is_empty(), "empty index should return no results");
}

#[test]
fn test_name_lookup_basic() {
    let (conn, provider) = setup();

    let nr = NameRecord {
        schema_version: 1,
        record_id: "nr-001".into(),
        normalized_name: normalize_name("My-Service"),
        original_name: "My-Service".into(),
        record_type: NameRecordType::Uri,
        target: "ant://deadbeef01234567890123456789012345678901234567890123456789012345".into(),
        target_scheme: Some("ant".into()),
        target_canonical: Some(
            "ant://deadbeef01234567890123456789012345678901234567890123456789012345".into(),
        ),
        ttl_secs: 3600,
        by_agent_id: "agent-abc".into(),
        origin_agent_id: None,
        timestamp: 1_700_000_000,
        signature: vec![],
    };
    repo::names::insert(&conn, &nr).expect("insert name record");

    let index = LocalIndex::new(&conn, &provider);

    // Case-insensitive lookup
    let scheme_filter = SchemeFilter::default(); // allow all
    let results = index
        .resolve_name("my-service", &scheme_filter)
        .expect("lookup");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].record.record_id, "nr-001");

    // Case variation — should still find it
    let results2 = index
        .resolve_name("MY-SERVICE", &scheme_filter)
        .expect("lookup");
    assert_eq!(results2.len(), 1);
}

#[test]
fn test_name_lookup_scheme_filter() {
    let (conn, provider) = setup();

    // Insert two records for same name, different schemes
    let nr_ant = NameRecord {
        schema_version: 1,
        record_id: "nr-ant".into(),
        normalized_name: normalize_name("my-app"),
        original_name: "my-app".into(),
        record_type: NameRecordType::Uri,
        target: "ant://deadbeef01234567890123456789012345678901234567890123456789012345".into(),
        target_scheme: Some("ant".into()),
        target_canonical: Some(
            "ant://deadbeef01234567890123456789012345678901234567890123456789012345".into(),
        ),
        ttl_secs: 3600,
        by_agent_id: "agent-a".into(),
        origin_agent_id: None,
        timestamp: 1_700_000_000,
        signature: vec![],
    };
    let nr_https = NameRecord {
        schema_version: 1,
        record_id: "nr-https".into(),
        normalized_name: normalize_name("my-app"),
        original_name: "my-app".into(),
        record_type: NameRecordType::Uri,
        target: "https://example.com/my-app".into(),
        target_scheme: Some("https".into()),
        target_canonical: Some("https://example.com/my-app".into()),
        ttl_secs: 3600,
        by_agent_id: "agent-b".into(),
        origin_agent_id: None,
        timestamp: 1_700_000_001,
        signature: vec![],
    };
    repo::names::insert(&conn, &nr_ant).expect("insert ant");
    repo::names::insert(&conn, &nr_https).expect("insert https");

    let index = LocalIndex::new(&conn, &provider);

    // Filter to autonomi only
    let results = index
        .resolve_name("my-app", &SchemeFilter::new(["autonomi"]))
        .expect("lookup");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].record.record_id, "nr-ant");

    // Allow all
    let results_all = index
        .resolve_name("my-app", &SchemeFilter::default())
        .expect("lookup");
    assert_eq!(results_all.len(), 2);
}

#[test]
fn test_name_lookup_not_found() {
    let (conn, provider) = setup();
    let index = LocalIndex::new(&conn, &provider);
    let results = index
        .resolve_name("nonexistent", &SchemeFilter::default())
        .expect("lookup");
    assert!(results.is_empty());
}

#[test]
fn test_mime_filter() {
    let (conn, provider) = setup();

    store_resource(
        &conn,
        &provider,
        JPEG_BYTES,
        "fish.jpg",
        "https://example.com/fish.jpg",
    );
    store_resource(
        &conn,
        &provider,
        TEXT_BYTES,
        "notes.txt",
        "https://example.com/notes.txt",
    );

    let index = LocalIndex::new(&conn, &provider);
    let mut filter = QueryFilter::default();
    filter.mime = MimeFilter::new(["image/"]);
    let results = index.search("file", 10, &filter).expect("search");

    for r in &results {
        assert!(
            r.resource.mime_type.starts_with("image/"),
            "mime filter should only return image/* resources, got {}",
            r.resource.mime_type
        );
    }
}

// ── Phase 7: Ranking shift tests ─────────────────────────────────────────────

#[test]
fn test_ranking_shifts_with_positive_feedback() {
    let (conn, provider) = setup();

    // Insert two text resources with different content so they get different mock vectors
    store_resource(
        &conn,
        &provider,
        b"document about rust programming language",
        "rust.txt",
        "https://example.com/rust.txt",
    );
    store_resource(
        &conn,
        &provider,
        b"document about python scripting language",
        "python.txt",
        "https://example.com/python.txt",
    );

    let index = LocalIndex::new(&conn, &provider);
    let filter = QueryFilter::default();

    // Baseline scores (no feedback yet)
    let results_before = index
        .search("programming language", 10, &filter)
        .expect("search");
    assert_eq!(results_before.len(), 2);

    // Find which resource is "rust.txt"
    let rust_resource = repo::resources::list(&conn)
        .unwrap()
        .into_iter()
        .find(|r| r.filename.as_deref() == Some("rust.txt"))
        .unwrap();

    // Add two positive feedbacks on rust.txt
    for i in 0..2u32 {
        let fb = av_core::types::FeedbackEvent {
            schema_version: 1,
            feedback_id: format!("fb-{i}"),
            query_text: "programming language".into(),
            resource_id: rust_resource.id.clone(),
            by_agent_id: format!("agent-{i}"),
            kind: av_core::types::FeedbackKind::Useful,
            timestamp: 1_700_000_000 + i as i64,
            signature: vec![],
        };
        repo::feedback::insert(&conn, &fb).unwrap();
    }

    // Also add a claim from a second agent to boost agreement
    let claim = av_core::types::Claim {
        schema_version: 1,
        claim_id: "claim-rust-001".into(),
        subject: rust_resource.id.clone(),
        predicate: "about".into(),
        object: "rust programming".into(),
        by_agent_id: "agent-external".into(),
        timestamp: 1_700_000_001,
        signature: vec![],
    };
    repo::claims::insert(&conn, &claim).unwrap();

    // Re-search: rust.txt should now score higher due to feedback + agreement
    let results_after = index
        .search("programming language", 10, &filter)
        .expect("search");
    assert_eq!(results_after.len(), 2);

    let rust_score_after = results_after
        .iter()
        .find(|r| r.resource.filename.as_deref() == Some("rust.txt"))
        .map(|r| r.score)
        .unwrap();
    let rust_score_before = results_before
        .iter()
        .find(|r| r.resource.filename.as_deref() == Some("rust.txt"))
        .map(|r| r.score)
        .unwrap();

    assert!(
        rust_score_after > rust_score_before,
        "positive feedback + agreement should increase score: before={rust_score_before:.4} after={rust_score_after:.4}"
    );
}

#[test]
fn test_search_result_has_all_score_components() {
    let (conn, provider) = setup();
    store_resource(
        &conn,
        &provider,
        TEXT_BYTES,
        "doc.txt",
        "https://example.com/doc.txt",
    );

    let index = LocalIndex::new(&conn, &provider);
    let results = index
        .search("document", 5, &QueryFilter::default())
        .expect("search");

    assert!(!results.is_empty());
    let r = &results[0];
    // All component scores should be in [0, 1]
    assert!(r.semantic_score >= 0.0 && r.semantic_score <= 1.0);
    assert!(r.agreement_score >= 0.0 && r.agreement_score <= 1.0);
    assert!(r.feedback_score >= 0.0 && r.feedback_score <= 1.0);
    assert!(r.trust_score >= 0.0 && r.trust_score <= 1.0);
    assert!(r.score >= 0.0 && r.score <= 1.0);
}

// --- Local block policy ----------------------------------------------------
//
// A block is the operator's "I do not want this in my results" switch, so the
// filter has to hold on the main search path. These tests pin the behaviour at
// the lowest layer that every caller (CLI search, `av listen` responder, web
// search) depends on.

fn block(conn: &rusqlite::Connection, kind: repo::blocks::BlockKind, id: &str) {
    repo::blocks::add(conn, kind, id, Some("test"), 1_700_000_000, "test").expect("add block");
}

#[test]
fn test_blocked_resource_is_excluded_from_search() {
    let (conn, provider) = setup();

    let blocked = ingest_bytes(
        TEXT_BYTES,
        Some("blocked.txt"),
        "https://example.com/blocked.txt",
    )
    .expect("ingest");
    repo::resources::insert(&conn, &blocked).expect("insert");

    // Same store_resource pipeline, but pinned to the id we are about to block.
    let profile = provider.profile().clone();
    let pid = profile_id(&profile);
    repo::embeddings::insert_profile(&conn, &pid, &profile).expect("insert profile");
    let rec = provider
        .embed_resource(&blocked.description_text, &blocked.id)
        .expect("embed");
    repo::embeddings::insert(&conn, &rec).expect("insert embedding");

    let index = LocalIndex::new(&conn, &provider);
    let before = index
        .search("document", 5, &QueryFilter::default())
        .expect("search");
    assert!(
        before.iter().any(|r| r.resource.id == blocked.id),
        "control: the resource must be searchable before it is blocked"
    );

    block(&conn, repo::blocks::BlockKind::Resource, &blocked.id);

    // The row is still there — a block hides, it does not have to delete.
    assert!(
        repo::resources::get(&conn, &blocked.id)
            .expect("get")
            .is_some()
    );

    let after = index
        .search("document", 5, &QueryFilter::default())
        .expect("search");
    assert!(
        !after.iter().any(|r| r.resource.id == blocked.id),
        "a blocked resource must not appear in search results"
    );
}

#[test]
fn test_unblocking_a_resource_restores_it_to_search() {
    let (conn, provider) = setup();
    let resource =
        ingest_bytes(TEXT_BYTES, Some("doc.txt"), "https://example.com/doc.txt").expect("ingest");
    repo::resources::insert(&conn, &resource).expect("insert");
    let profile = provider.profile().clone();
    let pid = profile_id(&profile);
    repo::embeddings::insert_profile(&conn, &pid, &profile).expect("insert profile");
    let rec = provider
        .embed_resource(&resource.description_text, &resource.id)
        .expect("embed");
    repo::embeddings::insert(&conn, &rec).expect("insert embedding");

    let index = LocalIndex::new(&conn, &provider);
    block(&conn, repo::blocks::BlockKind::Resource, &resource.id);
    assert!(
        index
            .search("document", 5, &QueryFilter::default())
            .expect("search")
            .is_empty()
    );

    repo::blocks::remove(&conn, repo::blocks::BlockKind::Resource, &resource.id).expect("unblock");

    let after = index
        .search("document", 5, &QueryFilter::default())
        .expect("search");
    assert!(
        after.iter().any(|r| r.resource.id == resource.id),
        "unblocking a resource that was never deleted must bring it back"
    );
}

#[test]
fn test_blocked_name_record_does_not_resolve() {
    let (conn, provider) = setup();

    let nr = NameRecord {
        schema_version: 1,
        record_id: "nr-blocked".into(),
        normalized_name: normalize_name("Blocked-Service"),
        original_name: "Blocked-Service".into(),
        record_type: NameRecordType::Uri,
        target: "autonomi://deadbeef01234567890123456789012345678901234567890123456789012345"
            .into(),
        target_scheme: Some("autonomi".into()),
        target_canonical: Some(
            "autonomi://deadbeef01234567890123456789012345678901234567890123456789012345".into(),
        ),
        ttl_secs: 3600,
        by_agent_id: "agent-abc".into(),
        origin_agent_id: None,
        timestamp: 1_700_000_000,
        signature: vec![],
    };
    repo::names::insert(&conn, &nr).expect("insert name record");

    let index = LocalIndex::new(&conn, &provider);
    let scheme_filter = SchemeFilter::default();

    let before = index
        .resolve_name("blocked-service", &scheme_filter)
        .expect("lookup");
    assert_eq!(
        before.len(),
        1,
        "control: the name must resolve before it is blocked"
    );

    block(&conn, repo::blocks::BlockKind::Name, "nr-blocked");

    let after = index
        .resolve_name("blocked-service", &scheme_filter)
        .expect("lookup");
    assert!(
        after.is_empty(),
        "a blocked name record must not resolve, in any case variant"
    );
    // The row survives: a block hides the record rather than destroying it, so
    // unblocking can bring it back without re-fetching from the network.
    assert!(
        repo::names::get_by_normalized_name(&conn, &normalize_name("Blocked-Service"))
            .expect("get")
            .iter()
            .any(|r| r.record_id == "nr-blocked"),
        "blocking a name must not delete the record"
    );
}

/// Put a `resources` row at a real location, so a name record has something to
/// resolve against.
///
/// The name-to-resource bridge is the whole point of these tests, so the row has
/// to be built the way production builds it: `resources.location` is the
/// normalized URI, and that is the string a name's `target` is matched against.
fn index_resource_at(conn: &rusqlite::Connection, id: &str, location: &str) {
    use av_core::types::{ResourceDescriptor, ResourceKind};

    let descriptor = ResourceDescriptor {
        id: id.to_string(),
        kind: ResourceKind::Text,
        location: location.to_string(),
        location_scheme: Some(location.split(':').next().unwrap_or("").to_string()),
        location_canonical: Some(location.to_string()),
        mime_type: "text/plain".into(),
        filename: None,
        metadata_json: serde_json::json!({}),
        description_text: "a description".into(),
        created_at: 1_700_000_000,
    };
    repo::resources::insert(conn, &descriptor).expect("insert resource");
}

#[test]
fn test_blocking_one_name_record_does_not_hide_a_conflicting_record() {
    let (conn, provider) = setup();

    // Two records claim the same name, as happens when two agents both claim it.
    // Names resolve by exact normalized match, so there is no ranking to
    // disambiguate them: if a block hid "the name", it would hide the innocent
    // claim too. The block is per record id, which is the only way to keep the
    // two independent.
    for (id, agent) in [("nr-good", "agent-good"), ("nr-bad", "agent-bad")] {
        repo::names::insert(
            &conn,
            &NameRecord {
                schema_version: 1,
                record_id: id.into(),
                normalized_name: normalize_name("Contested"),
                original_name: "Contested".into(),
                record_type: NameRecordType::Uri,
                target: "autonomi://aa".into(),
                target_scheme: Some("autonomi".into()),
                target_canonical: Some("autonomi://aa".into()),
                ttl_secs: 3600,
                by_agent_id: agent.into(),
                origin_agent_id: None,
                timestamp: 1_700_000_000,
                signature: vec![],
            },
        )
        .expect("insert name record");
    }

    block(&conn, repo::blocks::BlockKind::Name, "nr-bad");

    let index = LocalIndex::new(&conn, &provider);
    let results = index
        .resolve_name("contested", &SchemeFilter::default())
        .expect("lookup");
    let ids: Vec<&str> = results
        .iter()
        .map(|r| r.record.record_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["nr-good"],
        "only the blocked record is hidden; the other claim still resolves"
    );
}

/// A 64-hex agent id, which is the only shape `av block --agent` accepts.
fn agent_id(seed: char) -> String {
    seed.to_string().repeat(64)
}

/// Store a name record as if it had been cached from `origin`, with
/// `by_agent_id` set independently so tests can make the two disagree.
fn cache_name_from(
    conn: &rusqlite::Connection,
    record_id: &str,
    name: &str,
    target: &str,
    by_agent_id: &str,
    origin: Option<&str>,
) {
    repo::names::insert_with_origin(
        conn,
        &NameRecord {
            schema_version: 1,
            record_id: record_id.into(),
            normalized_name: normalize_name(name),
            original_name: name.into(),
            record_type: NameRecordType::Uri,
            target: target.into(),
            target_scheme: Some("autonomi".into()),
            target_canonical: Some(target.into()),
            ttl_secs: 3600,
            by_agent_id: by_agent_id.into(),
            origin_agent_id: origin.map(str::to_string),
            timestamp: 1_700_000_000,
            signature: vec![],
        },
        origin,
    )
    .expect("insert name record");
}

#[test]
fn a_name_cached_from_a_blocked_peer_stops_resolving() {
    let (conn, provider) = setup();

    cache_name_from(
        &conn,
        "nr-from-bad",
        "shared",
        "autonomi://bb",
        &agent_id('b'),
        Some(&agent_id('b')),
    );
    let index = LocalIndex::new(&conn, &provider);
    assert_eq!(
        index
            .resolve_name("shared", &SchemeFilter::default())
            .expect("lookup")
            .len(),
        1,
        "control: the name must resolve before the peer is blocked"
    );

    block(&conn, repo::blocks::BlockKind::Agent, &agent_id('b'));

    assert!(
        index
            .resolve_name("shared", &SchemeFilter::default())
            .expect("lookup")
            .is_empty(),
        "a name the blocked peer published must stop resolving"
    );
    // The row survives, like every other kind of block. Unblocking restores it
    // without another round trip.
    assert!(
        repo::names::get_by_normalized_name(&conn, "shared")
            .expect("get")
            .iter()
            .any(|r| r.record_id == "nr-from-bad"),
        "blocking a peer must not delete the names it published"
    );
}

#[test]
fn a_peer_cannot_hide_behind_an_id_it_wrote_itself() {
    let (conn, provider) = setup();

    // The peer claims to be `honest`, but x0x authenticated it as `liar`. Only
    // the second is usable: `by_agent_id` is inside the payload the sender chose,
    // so a block keyed on it is opt-out.
    cache_name_from(
        &conn,
        "nr-liar",
        "sneaky",
        "autonomi://cc",
        &agent_id('h'),
        Some(&agent_id('l')),
    );

    let index = LocalIndex::new(&conn, &provider);
    block(&conn, repo::blocks::BlockKind::Agent, &agent_id('l'));

    assert!(
        index
            .resolve_name("sneaky", &SchemeFilter::default())
            .expect("lookup")
            .is_empty(),
        "the block must follow the authenticated origin, not the self-reported id"
    );
}

#[test]
fn blocking_a_claimed_id_does_not_silence_a_different_real_origin() {
    let (conn, provider) = setup();

    // The mirror of the previous test: the record claims to be `liar` but
    // actually came from `honest`. Blocking the claimed id must leave it alone,
    // or blocking one peer would silence unrelated names.
    cache_name_from(
        &conn,
        "nr-honest",
        "sneaky",
        "autonomi://dd",
        &agent_id('l'),
        Some(&agent_id('h')),
    );

    let index = LocalIndex::new(&conn, &provider);
    block(&conn, repo::blocks::BlockKind::Agent, &agent_id('l'));

    assert_eq!(
        index
            .resolve_name("sneaky", &SchemeFilter::default())
            .expect("lookup")
            .len(),
        1,
        "a self-reported id must not be enough to censor a record"
    );
}

#[test]
fn blocking_every_peer_leaves_locally_authored_names_alone() {
    let (conn, provider) = setup();

    // `av name` writes records with no peer behind them. Blocking peers must not
    // take the local namespace with them, or a user could silence their own
    // names by blocking an unrelated agent.
    cache_name_from(
        &conn,
        "nr-mine",
        "mine",
        "autonomi://ee",
        &agent_id('m'),
        None,
    );

    let index = LocalIndex::new(&conn, &provider);
    for seed in ['a', 'b', 'c'] {
        block(&conn, repo::blocks::BlockKind::Agent, &agent_id(seed));
    }

    assert_eq!(
        index
            .resolve_name("mine", &SchemeFilter::default())
            .expect("lookup")
            .len(),
        1,
        "a record with no origin has no peer to block and must keep resolving"
    );
}

#[test]
fn blocking_a_peer_only_hides_its_own_claims() {
    let (conn, provider) = setup();

    // Two peers claim the same name, as happens whenever a name is contested.
    // Names resolve by exact normalized match with no ranking, so an agent block
    // has to be per record — otherwise blocking one peer censors the other's
    // perfectly good claim too.
    cache_name_from(
        &conn,
        "nr-from-bad",
        "contested",
        "autonomi://ff",
        &agent_id('b'),
        Some(&agent_id('b')),
    );
    cache_name_from(
        &conn,
        "nr-from-good",
        "contested",
        "autonomi://11",
        &agent_id('g'),
        Some(&agent_id('g')),
    );

    let index = LocalIndex::new(&conn, &provider);
    block(&conn, repo::blocks::BlockKind::Agent, &agent_id('b'));

    let ids: Vec<String> = index
        .resolve_name("contested", &SchemeFilter::default())
        .expect("lookup")
        .iter()
        .map(|r| r.record.record_id.clone())
        .collect();
    assert_eq!(
        ids,
        vec!["nr-from-good"],
        "only the blocked peer's claim is hidden"
    );
}

#[test]
fn an_unreadable_block_list_serves_no_names() {
    let (conn, provider) = setup();

    cache_name_from(
        &conn,
        "nr-1",
        "svc",
        "autonomi://22",
        &agent_id('a'),
        Some(&agent_id('a')),
    );
    // Prove the name resolves while the database is intact.
    let index = LocalIndex::new(&conn, &provider);
    assert_eq!(
        index
            .resolve_name("svc", &SchemeFilter::default())
            .expect("lookup")
            .len(),
        1
    );

    // Drop the table the way a corrupted or half-migrated database would. A read
    // that errors is not evidence that nothing is blocked, so the lookup has to
    // fail rather than return the name.
    conn.execute_batch("DROP TABLE blocks")
        .expect("drop blocks");

    assert!(
        index.resolve_name("svc", &SchemeFilter::default()).is_err(),
        "a failed block read must not read as 'nothing is blocked'"
    );
}

#[test]
fn blocking_a_resource_suppresses_the_name_that_points_at_it() {
    let (conn, provider) = setup();

    index_resource_at(&conn, "res-blocked", "autonomi://example/blocked");
    index_resource_at(&conn, "res-kept", "autonomi://example/kept");

    {
        use av_core::types::NameRecordType;
        for (id, name, target) in [
            ("nr-blocked", "blocked-svc", "autonomi://example/blocked"),
            ("nr-kept", "kept-svc", "autonomi://example/kept"),
        ] {
            repo::names::insert(
                &conn,
                &NameRecord {
                    schema_version: 1,
                    record_id: id.into(),
                    normalized_name: normalize_name(name),
                    original_name: name.into(),
                    record_type: NameRecordType::Uri,
                    target: target.into(),
                    target_scheme: Some("autonomi".into()),
                    target_canonical: Some(target.into()),
                    ttl_secs: 3600,
                    by_agent_id: "agent-abc".into(),
                    origin_agent_id: None,
                    timestamp: 1_700_000_000,
                    signature: vec![],
                },
            )
            .expect("insert name record");
        }
    }

    let index = LocalIndex::new(&conn, &provider);
    assert_eq!(
        index
            .resolve_name("blocked-svc", &SchemeFilter::default())
            .expect("lookup")
            .len(),
        1,
        "control: the name resolves while its resource is unblocked"
    );

    block(&conn, repo::blocks::BlockKind::Resource, "res-blocked");

    assert!(
        index
            .resolve_name("blocked-svc", &SchemeFilter::default())
            .expect("lookup")
            .is_empty(),
        "a name must not hand out a resource the operator blocked"
    );
    assert_eq!(
        index
            .resolve_name("kept-svc", &SchemeFilter::default())
            .expect("lookup")
            .len(),
        1,
        "blocking one resource must not censor an unrelated name"
    );
}

#[test]
fn a_name_reaches_its_resource_through_the_legacy_scheme_alias() {
    let (conn, provider) = setup();

    // The row is stored under the canonical scheme, as `av index` does, but the
    // record arrived from a peer still carrying `ant://`. The bridge has to try
    // `target_canonical` too, or a legacy record would slip past the block and
    // hand out the very link that was blocked.
    index_resource_at(&conn, "res-a", "autonomi://example/doc");

    use av_core::types::NameRecordType;
    repo::names::insert(
        &conn,
        &NameRecord {
            schema_version: 1,
            record_id: "nr-legacy".into(),
            normalized_name: normalize_name("legacy-svc"),
            original_name: "legacy-svc".into(),
            record_type: NameRecordType::Uri,
            target: "ant://example/doc".into(),
            target_scheme: Some("ant".into()),
            target_canonical: Some("autonomi://example/doc".into()),
            ttl_secs: 3600,
            by_agent_id: "agent-abc".into(),
            origin_agent_id: None,
            timestamp: 1_700_000_000,
            signature: vec![],
        },
    )
    .expect("insert name record");

    let index = LocalIndex::new(&conn, &provider);
    block(&conn, repo::blocks::BlockKind::Resource, "res-a");

    assert!(
        index
            .resolve_name("legacy-svc", &SchemeFilter::default())
            .expect("lookup")
            .is_empty(),
        "the target must be matched through the canonical scheme"
    );
}

#[test]
fn blocking_a_resource_the_name_does_not_hold_leaves_the_namespace_intact() {
    let (conn, provider) = setup();

    // A name whose target this machine never held. There is nothing to match the
    // block against, so the record stays resolvable. Dropping it instead would
    // let one block censor every name in the local namespace that points
    // somewhere else.
    use av_core::types::NameRecordType;
    repo::names::insert(
        &conn,
        &NameRecord {
            schema_version: 1,
            record_id: "nr-elsewhere".into(),
            normalized_name: normalize_name("elsewhere"),
            original_name: "elsewhere".into(),
            record_type: NameRecordType::Uri,
            target: "autonomi://not-held-locally".into(),
            target_scheme: Some("autonomi".into()),
            target_canonical: Some("autonomi://not-held-locally".into()),
            ttl_secs: 3600,
            by_agent_id: "agent-abc".into(),
            origin_agent_id: None,
            timestamp: 1_700_000_000,
            signature: vec![],
        },
    )
    .expect("insert name record");

    let index = LocalIndex::new(&conn, &provider);
    block(
        &conn,
        repo::blocks::BlockKind::Resource,
        "res-something-else",
    );

    assert_eq!(
        index
            .resolve_name("elsewhere", &SchemeFilter::default())
            .expect("lookup")
            .len(),
        1,
        "an unresolvable target is not the same as a blocked one"
    );
}

#[test]
fn a_broken_resources_table_fails_closed_rather_than_serving_the_name() {
    let (conn, provider) = setup();

    index_resource_at(&conn, "res-a", "autonomi://example/doc");

    use av_core::types::NameRecordType;
    repo::names::insert(
        &conn,
        &NameRecord {
            schema_version: 1,
            record_id: "nr-1".into(),
            normalized_name: normalize_name("svc"),
            original_name: "svc".into(),
            record_type: NameRecordType::Uri,
            target: "autonomi://example/doc".into(),
            target_scheme: Some("autonomi".into()),
            target_canonical: Some("autonomi://example/doc".into()),
            ttl_secs: 3600,
            by_agent_id: "agent-abc".into(),
            origin_agent_id: None,
            timestamp: 1_700_000_000,
            signature: vec![],
        },
    )
    .expect("insert name record");

    let index = LocalIndex::new(&conn, &provider);
    block(&conn, repo::blocks::BlockKind::Resource, "res-a");

    // With a resource block in force the bridge has to consult `resources`.
    // If that read fails, treating the error as "target not held locally" would
    // keep the record and serve the exact link the block exists to hide.
    conn.execute_batch("DROP TABLE resources")
        .expect("drop resources");

    assert!(
        index.resolve_name("svc", &SchemeFilter::default()).is_err(),
        "a failed target lookup must not be read as 'nothing to suppress'"
    );
}
