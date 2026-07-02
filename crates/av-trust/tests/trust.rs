use av_core::types::{FeedbackEvent, FeedbackKind};
use av_store::{open_in_memory, repo};
use av_trust::{
    agreement::agreement_score,
    decay::{DEFAULT_DECAY_RATE, apply_decay},
    feedback::feedback_score,
    ranking::search_score,
    update::{apply_negative, apply_positive, new_neutral},
};

fn setup() -> rusqlite::Connection {
    open_in_memory().expect("open db")
}

// ── Agreement ────────────────────────────────────────────────────────────────

#[test]
fn test_agreement_no_claims_returns_neutral() {
    let conn = setup();
    let score = agreement_score(&conn, "res-001").expect("score");
    assert!((score - 0.5).abs() < 0.001, "no claims → neutral 0.5");
}

#[test]
fn test_agreement_single_agent_single_resource() {
    let conn = setup();
    insert_claim(&conn, "res-001", "agent-a");
    let score = agreement_score(&conn, "res-001").expect("score");
    // 1 agent claimed this / 1 total agent = 1.0
    assert!((score - 1.0).abs() < 0.001);
}

#[test]
fn test_agreement_partial() {
    let conn = setup();
    // agent-a and agent-b both exist; only agent-a claimed res-001
    insert_claim(&conn, "res-001", "agent-a");
    insert_claim(&conn, "res-002", "agent-b");
    let score = agreement_score(&conn, "res-001").expect("score");
    // 1 agent claimed res-001 / 2 total = 0.5
    assert!((score - 0.5).abs() < 0.001);
}

// ── Feedback ─────────────────────────────────────────────────────────────────

#[test]
fn test_feedback_no_events_returns_neutral() {
    let conn = setup();
    let score = feedback_score(&conn, "res-001").expect("score");
    assert!((score - 0.5).abs() < 0.001, "no feedback → neutral 0.5");
}

#[test]
fn test_feedback_all_useful() {
    let conn = setup();
    insert_feedback(&conn, "res-001", "fb-1", FeedbackKind::Useful);
    insert_feedback(&conn, "res-001", "fb-2", FeedbackKind::Useful);
    let score = feedback_score(&conn, "res-001").expect("score");
    assert!(score > 0.5, "all useful → score above neutral, got {score}");
}

#[test]
fn test_feedback_all_incorrect() {
    let conn = setup();
    insert_feedback(&conn, "res-001", "fb-1", FeedbackKind::Incorrect);
    insert_feedback(&conn, "res-001", "fb-2", FeedbackKind::Incorrect);
    let score = feedback_score(&conn, "res-001").expect("score");
    assert!(
        score < 0.5,
        "all incorrect → score below neutral, got {score}"
    );
}

#[test]
fn test_feedback_mixed() {
    let conn = setup();
    insert_feedback(&conn, "res-001", "fb-1", FeedbackKind::Useful);
    insert_feedback(&conn, "res-001", "fb-2", FeedbackKind::NotUseful);
    let score = feedback_score(&conn, "res-001").expect("score");
    // (+1.0 - 0.5) / 2 normalised = should be slightly above 0.5
    assert!(score > 0.0 && score < 1.0);
}

// ── Trust update ─────────────────────────────────────────────────────────────

#[test]
fn test_new_neutral_is_zero() {
    let state = new_neutral("agent-x");
    assert_eq!(state.trust_score, 0.0);
    assert_eq!(state.evidence_count, 0);
}

#[test]
fn test_apply_positive_increases_score() {
    let mut state = new_neutral("agent-x");
    apply_positive(&mut state, 1.0);
    assert!(
        state.trust_score > 0.0,
        "positive evidence should increase score"
    );
    assert_eq!(state.evidence_count, 1);
}

#[test]
fn test_apply_negative_decreases_score() {
    let mut state = new_neutral("agent-x");
    apply_negative(&mut state, 1.0);
    assert!(
        state.trust_score < 0.0,
        "negative evidence should decrease score"
    );
    assert_eq!(state.evidence_count, 1);
}

#[test]
fn test_trust_bounded_positive() {
    let mut state = new_neutral("agent-x");
    for _ in 0..1000 {
        apply_positive(&mut state, 1.0);
    }
    assert!(state.trust_score <= 1.0, "trust must not exceed 1.0");
}

#[test]
fn test_trust_bounded_negative() {
    let mut state = new_neutral("agent-x");
    for _ in 0..1000 {
        apply_negative(&mut state, 1.0);
    }
    assert!(state.trust_score >= -1.0, "trust must not go below -1.0");
}

#[test]
fn test_decay_moves_toward_zero() {
    let mut state = new_neutral("agent-x");
    state.trust_score = 0.8;
    state.last_updated_at = 0; // far in the past
    apply_decay(&mut state, DEFAULT_DECAY_RATE);
    assert!(
        state.trust_score < 0.8,
        "decay should reduce score toward 0"
    );
    assert!(
        state.trust_score >= 0.0,
        "positive score stays positive after decay"
    );
}

// ── Full ranking formula ──────────────────────────────────────────────────────

#[test]
fn test_search_score_components_sum_to_combined() {
    let conn = setup();
    let components = search_score(&conn, "res-001", 0.8, None, None).expect("score");
    let expected = 0.55 * components.semantic
        + 0.15 * components.agreement
        + 0.10 * components.feedback
        + 0.10 * components.trust
        + 0.10 * components.relevance;
    assert!((components.combined - expected).abs() < 1e-5);
}

#[test]
fn test_ranking_shifts_after_positive_feedback() {
    let conn = setup();

    // Two resources with identical agreement/trust (no data)
    // res-A gets positive feedback, res-B gets none
    insert_feedback(&conn, "res-A", "fb-1", FeedbackKind::Useful);
    insert_feedback(&conn, "res-A", "fb-2", FeedbackKind::Useful);
    // res-B: no feedback

    let score_a = search_score(&conn, "res-A", 0.7, None, None).expect("a").combined;
    let score_b = search_score(&conn, "res-B", 0.7, None, None).expect("b").combined;

    assert!(
        score_a > score_b,
        "res-A with positive feedback ({score_a:.3}) should outscore res-B with none ({score_b:.3})"
    );
}

#[test]
fn test_ranking_shifts_after_negative_feedback() {
    let conn = setup();

    insert_feedback(&conn, "res-bad", "fb-1", FeedbackKind::Incorrect);
    insert_feedback(&conn, "res-bad", "fb-2", FeedbackKind::Incorrect);

    let score_bad = search_score(&conn, "res-bad", 0.7, None, None)
        .expect("bad")
        .combined;
    let score_good = search_score(&conn, "res-good", 0.7, None, None)
        .expect("good")
        .combined;

    assert!(
        score_good > score_bad,
        "res-bad with negative feedback ({score_bad:.3}) should score below neutral res-good ({score_good:.3})"
    );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn insert_claim(conn: &rusqlite::Connection, resource_id: &str, agent_id: &str) {
    use av_core::types::Claim;
    let claim = Claim {
        schema_version: 1,
        claim_id: format!("{resource_id}-{agent_id}"),
        subject: resource_id.to_string(),
        predicate: "about".to_string(),
        object: "test".to_string(),
        by_agent_id: agent_id.to_string(),
        timestamp: 1_700_000_000,
        signature: vec![],
    };
    repo::claims::insert(conn, &claim).expect("insert claim");
}

fn insert_feedback(
    conn: &rusqlite::Connection,
    resource_id: &str,
    fb_id: &str,
    kind: FeedbackKind,
) {
    // resource must exist in DB for FK constraint
    // For trust tests, skip FK by inserting a dummy resource first
    let _ = conn.execute(
        "INSERT OR IGNORE INTO resources
         (id, kind, location, location_scheme, location_canonical, mime_type, filename, metadata_json, description_text, created_at)
         VALUES (?1, 'text', 'https://x.com', 'https', NULL, 'text/plain', NULL, '{}', 'test', 0)",
        rusqlite::params![resource_id],
    );
    let fb = FeedbackEvent {
        schema_version: 1,
        feedback_id: fb_id.to_string(),
        query_text: "test query".to_string(),
        resource_id: resource_id.to_string(),
        by_agent_id: "agent-test".to_string(),
        kind,
        timestamp: 1_700_000_000,
        signature: vec![],
    };
    repo::feedback::insert(conn, &fb).expect("insert feedback");
}
