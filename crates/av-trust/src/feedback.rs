use crate::error::TrustResult;
use av_core::types::FeedbackKind;
use rusqlite::Connection;

/// Aggregate feedback for a resource into a single score in [0, 1].
/// Returns 0.5 (neutral) when no feedback exists.
pub fn feedback_score(conn: &Connection, resource_id: &str) -> TrustResult<f32> {
    let events = av_store::repo::feedback::list_by_resource(conn, resource_id)?;

    if events.is_empty() {
        return Ok(0.5); // neutral
    }

    let raw: f32 = events.iter().map(|e| signal_weight(&e.kind)).sum();

    // Normalise: raw per-event range is [-1, +1]; map to [0, 1] via midpoint shift
    let normalised = (raw / events.len() as f32 + 1.0) / 2.0;
    Ok(normalised.clamp(0.0, 1.0))
}

fn signal_weight(kind: &FeedbackKind) -> f32 {
    match kind {
        FeedbackKind::Useful => 1.0,
        FeedbackKind::HighConfidence => 1.0,
        FeedbackKind::NotUseful => -0.5,
        FeedbackKind::Incorrect => -1.0,
    }
}
