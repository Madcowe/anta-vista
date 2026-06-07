use av_core::types::TrustState;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Default decay rate — halves trust deviation from 0 every ~30 days.
/// ln(2) / (30 * 86400) ≈ 2.67e-7 per second.
pub const DEFAULT_DECAY_RATE: f32 = 2.67e-7;

/// Decay trust score toward 0 based on time elapsed since last update.
/// Uses exponential decay: score *= e^(-rate * elapsed_secs)
pub fn apply_decay(state: &mut TrustState, rate: f32) {
    let now = now_secs();
    let elapsed = (now - state.last_updated_at).max(0) as f32;
    if elapsed < 1.0 {
        return;
    }
    let factor = (-rate * elapsed).exp();
    state.trust_score *= factor;
    state.last_updated_at = now;
}

/// Apply decay to all trust states in the DB and persist changes.
pub fn decay_all(conn: &rusqlite::Connection, rate: f32) -> crate::error::TrustResult<usize> {
    let mut states = av_store::repo::trust::list_all(conn)?;
    for state in &mut states {
        apply_decay(state, rate);
        av_store::repo::trust::upsert(conn, state)?;
    }
    Ok(states.len())
}
