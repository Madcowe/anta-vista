use av_core::types::*;
use av_store::{open_in_memory, repo};

#[test]
fn test_full_integration() {
    let conn = open_in_memory().expect("open db");

    // Resource
    let resource = ResourceDescriptor {
        id: "res-001".into(),
        kind: ResourceKind::Text,
        location: "https://example.com/doc.txt".into(),
        location_scheme: Some("https".into()),
        location_canonical: None,
        mime_type: "text/plain".into(),
        filename: Some("doc.txt".into()),
        metadata_json: serde_json::json!({}),
        description_text: "a text document".into(),
        created_at: 1_700_000_000,
    };
    repo::resources::insert(&conn, &resource).unwrap();
    let fetched = repo::resources::get(&conn, "res-001").unwrap().unwrap();
    assert_eq!(fetched.id, "res-001");
    assert_eq!(fetched.location, "https://example.com/doc.txt");

    // Embedding profile
    let profile = EmbeddingProfile {
        model_id: "all-MiniLM-L6-v2".into(),
        model_version: "1.0.0".into(),
        dim: 384,
        normalized: true,
        preproc_version: "v1".into(),
    };
    repo::embeddings::insert_profile(&conn, "all-MiniLM-L6-v2:1.0.0", &profile).unwrap();

    // Embedding record
    let vector: Vec<f32> = vec![0.1_f32; 384];
    let embedding = EmbeddingRecord {
        resource_id: "res-001".into(),
        profile_id: "all-MiniLM-L6-v2:1.0.0".into(),
        vector: vector.clone(),
        l2_norm: 1.0,
        created_at: 1_700_000_001,
    };
    repo::embeddings::insert(&conn, &embedding).unwrap();
    let fetched_emb = repo::embeddings::get(&conn, "res-001", "all-MiniLM-L6-v2:1.0.0")
        .unwrap()
        .unwrap();
    assert_eq!(fetched_emb.vector.len(), 384);

    // Claim
    let claim = Claim {
        schema_version: 1,
        claim_id: "claim-001".into(),
        subject: "res-001".into(),
        predicate: "about".into(),
        object: "Rust programming".into(),
        by_agent_id: "agent-abc".into(),
        timestamp: 1_700_000_002,
        signature: vec![0xde, 0xad],
    };
    repo::claims::insert(&conn, &claim).unwrap();
    let claims = repo::claims::list_by_subject(&conn, "res-001").unwrap();
    assert_eq!(claims.len(), 1);

    // Feedback
    let fb = FeedbackEvent {
        schema_version: 1,
        feedback_id: "fb-001".into(),
        query_text: "rust docs".into(),
        resource_id: "res-001".into(),
        by_agent_id: "agent-abc".into(),
        kind: FeedbackKind::Useful,
        timestamp: 1_700_000_003,
        signature: vec![],
    };
    repo::feedback::insert(&conn, &fb).unwrap();
    let feedbacks = repo::feedback::list_by_resource(&conn, "res-001").unwrap();
    assert_eq!(feedbacks.len(), 1);

    // Trust
    let trust = TrustState {
        subject_agent_id: "agent-abc".into(),
        trust_score: 0.5,
        evidence_count: 1,
        last_updated_at: 1_700_000_004,
    };
    repo::trust::upsert(&conn, &trust).unwrap();
    let t = repo::trust::get(&conn, "agent-abc").unwrap().unwrap();
    assert!((t.trust_score - 0.5).abs() < 0.001);

    // Name record
    let nr = NameRecord {
        schema_version: 1,
        record_id: "nr-001".into(),
        normalized_name: "my-service".into(),
        original_name: "My-Service".into(),
        record_type: NameRecordType::Uri,
        target: "ant://deadbeef".into(),
        target_scheme: Some("ant".into()),
        target_canonical: Some("ant://deadbeef".into()),
        ttl_secs: 3600,
        by_agent_id: "agent-abc".into(),
        timestamp: 1_700_000_005,
        signature: vec![],
    };
    repo::names::insert(&conn, &nr).unwrap();
    let names = repo::names::get_by_normalized_name(&conn, "my-service").unwrap();
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].record_id, "nr-001");

    // Peer cache
    repo::peers::upsert(
        &conn,
        "peer-001",
        serde_json::json!({"addr": "1.2.3.4"}),
        1_700_000_006,
    )
    .unwrap();
    let peers = repo::peers::list_recent(&conn, 10).unwrap();
    assert_eq!(peers.len(), 1);
}
