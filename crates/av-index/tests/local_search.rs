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
