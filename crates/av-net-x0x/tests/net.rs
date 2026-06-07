use av_core::types::{MessageKind, NameRecord, NameRecordType, normalize_name};
use av_net_x0x::{
    DedupeCache, MessageDispatcher, MockNetClient, NetError, NetworkClient, build_envelope,
    payloads::{NameQueryPayload, QueryPayload},
    validate_envelope,
};
use std::sync::Arc;
use std::time::Duration;

// ── Envelope ─────────────────────────────────────────────────────────────────

#[test]
fn test_build_envelope_fields() {
    let env = build_envelope(
        "agent-abc",
        MessageKind::Query,
        serde_json::json!({"q": "hello"}),
    );
    assert_eq!(env.schema_version, 1);
    assert_eq!(env.from_agent_id, "agent-abc");
    assert!(!env.message_id.is_empty());
    assert!(env.sent_at > 0);
}

#[test]
fn test_validate_envelope_ok() {
    let env = build_envelope("agent-abc", MessageKind::Query, serde_json::json!({}));
    let raw = serde_json::to_vec(&env).unwrap();
    validate_envelope(&env, raw.len()).expect("valid envelope should pass");
}

#[test]
fn test_validate_envelope_too_large() {
    let env = build_envelope("agent-abc", MessageKind::Query, serde_json::json!({}));
    let result = validate_envelope(&env, 2 * 1024 * 1024); // 2 MiB > limit
    assert!(matches!(result, Err(NetError::TooLarge { .. })));
}

#[test]
fn test_validate_envelope_bad_version() {
    let mut env = build_envelope("agent-abc", MessageKind::Query, serde_json::json!({}));
    env.schema_version = 99;
    let result = validate_envelope(&env, 100);
    assert!(matches!(result, Err(NetError::UnsupportedVersion(99))));
}

// ── DedupeCache ───────────────────────────────────────────────────────────────

#[test]
fn test_dedupe_first_is_not_duplicate() {
    let mut cache = DedupeCache::new(Duration::from_secs(60));
    assert!(!cache.is_duplicate("msg-001"));
}

#[test]
fn test_dedupe_second_is_duplicate() {
    let mut cache = DedupeCache::new(Duration::from_secs(60));
    cache.is_duplicate("msg-001");
    assert!(cache.is_duplicate("msg-001"));
}

#[test]
fn test_dedupe_different_ids_not_duplicate() {
    let mut cache = DedupeCache::new(Duration::from_secs(60));
    cache.is_duplicate("msg-001");
    assert!(!cache.is_duplicate("msg-002"));
}

// ── MockNetClient ─────────────────────────────────────────────────────────────

#[test]
fn test_mock_publish_records() {
    let client = MockNetClient::new("agent-test");
    let env = build_envelope("agent-test", MessageKind::Query, serde_json::json!({}));
    client.publish("av.query.v1", &env).expect("publish");
    assert_eq!(client.published_count("av.query.v1"), 1);
}

#[test]
fn test_mock_subscribe_records() {
    let client = MockNetClient::new("agent-test");
    client.subscribe("av.query.v1").expect("subscribe");
    client
        .subscribe("av.query.v1")
        .expect("subscribe again (idempotent)");
    let subs = client.subscribed();
    assert_eq!(subs.iter().filter(|s| *s == "av.query.v1").count(), 1);
}

// ── MessageDispatcher ─────────────────────────────────────────────────────────

fn make_dispatcher(agent_id: &str) -> (MessageDispatcher, Arc<MockNetClient>) {
    let mock = Arc::new(MockNetClient::new(agent_id));
    let dispatcher = MessageDispatcher::new(mock.clone());
    (dispatcher, mock)
}

#[test]
fn test_subscribe_all() {
    let (dispatcher, mock) = make_dispatcher("agent-a");
    dispatcher.subscribe_all().expect("subscribe_all");
    let subs = mock.subscribed();
    assert!(subs.contains(&"av.query.v1".to_string()));
    assert!(subs.contains(&"av.name.query.v1".to_string()));
    assert!(subs.contains(&"av.name.claim.v1".to_string()));
    assert_eq!(subs.len(), 7); // all 7 topics
}

#[test]
fn test_publish_query() {
    let (dispatcher, mock) = make_dispatcher("agent-a");
    let query_id = dispatcher
        .publish_query("rust programming", 10, 1000, vec![])
        .expect("query");
    assert!(!query_id.is_empty());
    assert_eq!(mock.published_count("av.query.v1"), 1);
    // Check the envelope kind
    let published = mock.published();
    assert!(matches!(
        published[0].1.kind,
        av_core::types::MessageKind::Query
    ));
}

#[test]
fn test_publish_name_query() {
    let (dispatcher, mock) = make_dispatcher("agent-a");
    let query_id = dispatcher
        .publish_name_query("Alice.App", Some("Uri"), 10, 1200)
        .expect("name query");
    assert!(!query_id.is_empty());
    assert_eq!(mock.published_count("av.name.query.v1"), 1);
    // Verify normalized_name in payload
    let published = mock.published();
    let payload: NameQueryPayload = serde_json::from_value(published[0].1.payload.clone()).unwrap();
    assert_eq!(payload.normalized_name, "alice.app");
    assert_eq!(payload.name, "Alice.App");
}

#[test]
fn test_publish_name_claim() {
    let (dispatcher, mock) = make_dispatcher("agent-a");
    let record = NameRecord {
        schema_version: 1,
        record_id: "rec-001".into(),
        normalized_name: normalize_name("Alice.App"),
        original_name: "Alice.App".into(),
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
    dispatcher.publish_name_claim(record).expect("name claim");
    assert_eq!(mock.published_count("av.name.claim.v1"), 1);
}

#[test]
fn test_validate_incoming_deduplication() {
    let (dispatcher, _) = make_dispatcher("agent-a");
    let env = build_envelope("agent-b", MessageKind::Query, serde_json::json!({}));
    let raw = serde_json::to_vec(&env).unwrap();
    let size = raw.len();
    // First time: ok
    dispatcher
        .validate_incoming(&env, size)
        .expect("first should pass");
    // Second time: duplicate
    let result = dispatcher.validate_incoming(&env, size);
    assert!(matches!(result, Err(NetError::Duplicate(_))));
}

#[test]
fn test_payload_serialization_roundtrip() {
    let p = QueryPayload {
        query_id: "q-001".into(),
        query_text: "find rust tutorials".into(),
        max_results: 10,
        timeout_ms: 1000,
        allowed_schemes: vec!["https".into(), "ant".into()],
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: QueryPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(back.query_id, "q-001");
    assert_eq!(back.allowed_schemes, vec!["https", "ant"]);
}

/// Two-node integration test — requires two running x0x daemon instances.
/// Run with: cargo test -- --include-ignored
/// Two-agent direct-messaging integration test — requires a running x0x daemon.
/// Run with: cargo test -- --include-ignored
#[test]
#[ignore = "requires a running x0x daemon"]
fn test_two_node_direct_message() {
    // Not automated here — manual integration test.
}

#[test]
#[ignore = "requires two running x0x daemon instances"]
fn test_two_node_query_response() {
    // This would:
    // 1. Connect node A and node B each to their local x0x daemons
    // 2. Node B subscribes to av.query.v1
    // 3. Node A publishes a query
    // 4. Node B receives it and publishes a response
    // 5. Node A receives the response
    // Not automated here — manual integration test.
}

// ── NetworkClient trait usage via MockNetClient ────────────────────────────────

#[test]
fn test_mock_client_agent_id() {
    let client = MockNetClient::new("my-agent-hex");
    assert_eq!(client.agent_id(), "my-agent-hex");
}

// ── Direct messaging ──────────────────────────────────────────────────────────

#[test]
fn test_connect_agent_records() {
    use av_net_x0x::NetworkClient;
    let client = MockNetClient::new("agent-a");
    client.connect_agent("agent-b").expect("connect");
    client
        .connect_agent("agent-b")
        .expect("connect again (idempotent)");
    let connected = client.connected_agents();
    assert_eq!(connected.iter().filter(|id| *id == "agent-b").count(), 1);
}

#[test]
fn test_send_direct_records() {
    use av_net_x0x::NetworkClient;
    let client = MockNetClient::new("agent-a");
    let env = build_envelope(
        "agent-a",
        av_core::types::MessageKind::Query,
        serde_json::json!({}),
    );
    client.send_direct("agent-b", &env).expect("send direct");
    assert_eq!(client.direct_sent_count("agent-b"), 1);
    assert_eq!(client.direct_sent_count("agent-c"), 0);
}

#[test]
fn test_dispatcher_connect_agent() {
    let (dispatcher, mock) = make_dispatcher("agent-a");
    dispatcher.connect_agent("agent-b").expect("connect");
    assert!(mock.connected_agents().contains(&"agent-b".to_string()));
}

#[test]
fn test_dispatcher_send_direct_query() {
    let (dispatcher, mock) = make_dispatcher("agent-a");
    let query_id = dispatcher
        .send_direct_query("agent-b", "find rust tutorials", 5, 1000, vec![])
        .expect("direct query");
    assert!(!query_id.is_empty());
    assert_eq!(mock.direct_sent_count("agent-b"), 1);
    let sent = mock.direct_sent();
    assert!(matches!(sent[0].1.kind, av_core::types::MessageKind::Query));
}

#[test]
fn test_dispatcher_send_direct_response() {
    let (dispatcher, mock) = make_dispatcher("agent-a");
    dispatcher
        .send_direct_response("agent-b", "q-001", vec![])
        .expect("direct response");
    assert_eq!(mock.direct_sent_count("agent-b"), 1);
    let sent = mock.direct_sent();
    assert!(matches!(
        sent[0].1.kind,
        av_core::types::MessageKind::Response
    ));
}

#[test]
fn test_dispatcher_send_direct_name_query() {
    let (dispatcher, mock) = make_dispatcher("agent-a");
    let query_id = dispatcher
        .send_direct_name_query("agent-b", "Alice.App", Some("Uri"), 5, 1200)
        .expect("direct name query");
    assert!(!query_id.is_empty());
    assert_eq!(mock.direct_sent_count("agent-b"), 1);
    // Check normalized_name in payload
    let sent = mock.direct_sent();
    let payload: av_net_x0x::NameQueryPayload =
        serde_json::from_value(sent[0].1.payload.clone()).unwrap();
    assert_eq!(payload.normalized_name, "alice.app");
}

#[test]
fn test_dispatcher_send_direct_name_response() {
    let (dispatcher, mock) = make_dispatcher("agent-a");
    dispatcher
        .send_direct_name_response("agent-b", "q-001", "alice.app", vec![])
        .expect("direct name response");
    assert_eq!(mock.direct_sent_count("agent-b"), 1);
    let sent = mock.direct_sent();
    assert!(matches!(
        sent[0].1.kind,
        av_core::types::MessageKind::NameResponse
    ));
}

/// Gossip vs direct mode: same query published two ways.
#[test]
fn test_gossip_vs_direct_same_envelope_format() {
    let (dispatcher, mock) = make_dispatcher("agent-a");
    // Gossip
    dispatcher
        .publish_query("test query", 5, 500, vec![])
        .expect("gossip query");
    // Direct
    dispatcher
        .send_direct_query("agent-b", "test query", 5, 500, vec![])
        .expect("direct query");

    // Both should produce a Query-kind envelope
    let gossip = mock.published();
    let direct = mock.direct_sent();
    assert!(matches!(
        gossip[0].1.kind,
        av_core::types::MessageKind::Query
    ));
    assert!(matches!(
        direct[0].1.kind,
        av_core::types::MessageKind::Query
    ));
    // Both use schema_version = 1
    assert_eq!(gossip[0].1.schema_version, 1);
    assert_eq!(direct[0].1.schema_version, 1);
}
