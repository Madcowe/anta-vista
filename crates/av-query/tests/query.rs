use av_net_x0x::payloads::{ResourceResult, ResponsePayload};
use av_query::{
    AbuseConfig, AbuseTracker, NodeMetrics, PayloadGuard, QueryError, RateLimitConfig, RateLimiter,
    cluster_responses, needs_clustering,
};
use av_store::open_in_memory;

// ── Cold-start clustering ─────────────────────────────────────────────────────

fn make_result(resource_id: &str, score: f32) -> ResourceResult {
    ResourceResult {
        resource_id: resource_id.to_string(),
        description_text: format!("description of {resource_id}"),
        location: format!("https://example.com/{resource_id}"),
        location_scheme: Some("https".into()),
        mime_type: "text/plain".into(),
        score,
    }
}

fn make_response(query_id: &str, results: Vec<ResourceResult>) -> ResponsePayload {
    ResponsePayload {
        query_id: query_id.to_string(),
        results,
    }
}

#[test]
fn test_cluster_empty_responses() {
    let clustered = cluster_responses(&[]);
    assert!(clustered.is_empty());
}

#[test]
fn test_cluster_single_agent() {
    let resp = (
        "agent-a".to_string(),
        make_response(
            "q1",
            vec![make_result("res-001", 0.9), make_result("res-002", 0.7)],
        ),
    );
    let clustered = cluster_responses(&[resp]);
    assert_eq!(clustered.len(), 2);
    // With only one agent, all agreement counts are 1
    assert!(clustered.iter().all(|c| c.agreement_count == 1));
    // Sorted by avg_score: res-001 first
    assert_eq!(clustered[0].result.resource_id, "res-001");
}

#[test]
fn test_cluster_agreement_ranks_higher() {
    // agent-a and agent-b both return res-001
    // only agent-a returns res-002 (higher individual score)
    let resp_a = (
        "agent-a".to_string(),
        make_response(
            "q1",
            vec![
                make_result("res-001", 0.6),
                make_result("res-002", 0.95), // highest score but only 1 agent
            ],
        ),
    );
    let resp_b = (
        "agent-b".to_string(),
        make_response("q1", vec![make_result("res-001", 0.7)]),
    );

    let clustered = cluster_responses(&[resp_a, resp_b]);

    // res-001 has agreement_count=2, res-002 has agreement_count=1
    // res-001 should rank first despite lower individual score
    assert_eq!(clustered[0].result.resource_id, "res-001");
    assert_eq!(clustered[0].agreement_count, 2);
    assert_eq!(clustered[1].result.resource_id, "res-002");
    assert_eq!(clustered[1].agreement_count, 1);
}

#[test]
fn test_cluster_deduplicates_same_agent_duplicate() {
    // Same agent submitting same resource twice — should count as 1
    let resp_a1 = (
        "agent-a".to_string(),
        make_response("q1", vec![make_result("res-001", 0.8)]),
    );
    let resp_a2 = (
        "agent-a".to_string(),
        make_response("q1", vec![make_result("res-001", 0.9)]),
    );
    let clustered = cluster_responses(&[resp_a1, resp_a2]);
    assert_eq!(clustered.len(), 1);
    assert_eq!(clustered[0].agreement_count, 1, "same agent counted once");
}

#[test]
fn test_needs_clustering_true_when_few_trusted() {
    assert!(
        needs_clustering(0, 3),
        "0 trusted agents → needs clustering"
    );
    assert!(needs_clustering(2, 3), "2 < 3 → needs clustering");
}

#[test]
fn test_needs_clustering_false_when_enough_trusted() {
    assert!(!needs_clustering(3, 3), "3 >= 3 → no clustering needed");
    assert!(!needs_clustering(10, 3), "plenty of trusted agents");
}

// ── Rate limiter ──────────────────────────────────────────────────────────────

#[test]
fn test_rate_limiter_allows_within_burst() {
    let config = RateLimitConfig::new(5.0, 1.0);
    let mut limiter = RateLimiter::new(config);
    // 5 messages should be fine (within burst capacity)
    for _ in 0..5 {
        assert!(limiter.check_and_consume("agent-a"), "should be allowed");
    }
}

#[test]
fn test_rate_limiter_blocks_after_burst() {
    let config = RateLimitConfig::new(3.0, 0.01); // tiny refill rate
    let mut limiter = RateLimiter::new(config);
    // Drain the bucket
    for _ in 0..3 {
        limiter.check_and_consume("agent-a");
    }
    // Next one should be blocked
    assert!(
        !limiter.check_and_consume("agent-a"),
        "should be rate-limited"
    );
}

#[test]
fn test_rate_limiter_independent_per_agent() {
    let config = RateLimitConfig::new(2.0, 0.01);
    let mut limiter = RateLimiter::new(config);
    // Drain agent-a
    limiter.check_and_consume("agent-a");
    limiter.check_and_consume("agent-a");
    assert!(!limiter.check_and_consume("agent-a"), "agent-a blocked");
    // agent-b should still be fine
    assert!(limiter.check_and_consume("agent-b"), "agent-b unaffected");
}

// ── Payload guard ─────────────────────────────────────────────────────────────

#[test]
fn test_guard_allows_normal_message() {
    let limiter = RateLimiter::new(RateLimitConfig::new(10.0, 1.0));
    let mut guard = PayloadGuard::new(limiter);
    guard
        .check("agent-a", 1024)
        .expect("normal message should pass");
}

#[test]
fn test_guard_rejects_oversized_payload() {
    let limiter = RateLimiter::new(RateLimitConfig::new(10.0, 1.0));
    let mut guard = PayloadGuard::new(limiter).with_max_bytes(512);
    let result = guard.check("agent-a", 1024);
    assert!(matches!(result, Err(QueryError::TooLarge { .. })));
}

#[test]
fn test_guard_rejects_rate_limited_agent() {
    let limiter = RateLimiter::new(RateLimitConfig::new(1.0, 0.001)); // 1 message burst
    let mut guard = PayloadGuard::new(limiter);
    guard.check("agent-a", 100).expect("first ok");
    let result = guard.check("agent-a", 100);
    assert!(matches!(result, Err(QueryError::RateLimited(_))));
}

// ── Abuse tracker ─────────────────────────────────────────────────────────────

#[test]
fn test_abuse_no_strikes_initially() {
    let tracker = AbuseTracker::new(AbuseConfig::default());
    assert_eq!(tracker.strike_count("agent-x"), 0);
    assert!(!tracker.is_blocked("agent-x"));
}

#[test]
fn test_abuse_strike_reduces_trust() {
    let conn = open_in_memory().expect("db");
    let mut tracker = AbuseTracker::new(AbuseConfig::default());
    // First strike should succeed (below block threshold)
    tracker
        .record_strike(&conn, "agent-x")
        .expect("first strike ok");
    assert_eq!(tracker.strike_count("agent-x"), 1);
    // Trust should now exist and be negative
    let state = av_store::repo::trust::get(&conn, "agent-x")
        .unwrap()
        .unwrap();
    assert!(state.trust_score < 0.0, "trust reduced by strike");
}

#[test]
fn test_abuse_blocks_after_threshold() {
    let conn = open_in_memory().expect("db");
    let config = AbuseConfig {
        block_threshold: 3,
        penalty_weight: 0.3,
    };
    let mut tracker = AbuseTracker::new(config);
    // Issue strikes up to threshold
    tracker.record_strike(&conn, "bad-agent").ok();
    tracker.record_strike(&conn, "bad-agent").ok();
    let result = tracker.record_strike(&conn, "bad-agent");
    assert!(
        matches!(result, Err(QueryError::AgentBlocked(_))),
        "should be blocked after 3 strikes"
    );
    assert!(tracker.is_blocked("bad-agent"));
}

#[test]
fn test_abuse_reset_clears_strikes() {
    let conn = open_in_memory().expect("db");
    let mut tracker = AbuseTracker::new(AbuseConfig::default());
    tracker.record_strike(&conn, "agent-y").ok();
    tracker.reset("agent-y");
    assert_eq!(tracker.strike_count("agent-y"), 0);
    assert!(!tracker.is_blocked("agent-y"));
}

// ── Metrics ───────────────────────────────────────────────────────────────────

#[test]
fn test_metrics_counters() {
    let m = NodeMetrics::new();
    m.inc_received();
    m.inc_received();
    m.inc_accepted();
    m.inc_rate_limited();
    m.inc_strikes();
    m.inc_blocked();
    m.inc_clusters();
    m.inc_queries();

    let snap = m.snapshot();
    assert_eq!(snap.messages_received, 2);
    assert_eq!(snap.messages_accepted, 1);
    assert_eq!(snap.messages_rate_limited, 1);
    assert_eq!(snap.strikes_issued, 1);
    assert_eq!(snap.agents_blocked, 1);
    assert_eq!(snap.clusters_computed, 1);
    assert_eq!(snap.queries_issued, 1);
}

#[test]
fn test_metrics_thread_safe() {
    use std::sync::Arc;
    use std::thread;

    let m = NodeMetrics::new();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let m2 = Arc::clone(&m);
            thread::spawn(move || {
                m2.inc_received();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(m.snapshot().messages_received, 4);
}
