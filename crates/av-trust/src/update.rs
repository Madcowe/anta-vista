use av_core::types::TrustState;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Apply a positive evidence signal (agreement, useful feedback, consistent behaviour).
/// `weight` ∈ (0, 1] — how strong the signal is.
pub fn apply_positive(state: &mut TrustState, weight: f32) {
    // Diminishing returns as score approaches +1
    let delta = weight * (1.0 - state.trust_score) * 0.1;
    state.trust_score = (state.trust_score + delta).min(1.0);
    state.evidence_count += 1;
    state.last_updated_at = now_secs();
}

/// Apply a negative evidence signal (disagreement, incorrect feedback, spam).
/// `weight` ∈ (0, 1].
pub fn apply_negative(state: &mut TrustState, weight: f32) {
    // Diminishing returns as score approaches -1
    let delta = weight * (state.trust_score + 1.0) * 0.1;
    state.trust_score = (state.trust_score - delta).max(-1.0);
    state.evidence_count += 1;
    state.last_updated_at = now_secs();
}

/// Create a fresh neutral TrustState for an agent never seen before.
pub fn new_neutral(agent_id: &str) -> TrustState {
    TrustState {
        subject_agent_id: agent_id.to_string(),
        trust_score: 0.0,
        evidence_count: 0,
        last_updated_at: now_secs(),
    }
}
