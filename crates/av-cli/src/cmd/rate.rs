use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use av_core::constants::TOPIC_FEEDBACK;
use av_core::types::{FeedbackEvent, FeedbackKind, MessageKind};
use av_net_x0x::NetworkClient;
use av_net_x0x::build_envelope;
use av_net_x0x::client::X0xNetClient;
use av_net_x0x::payloads::FeedbackPayload;
use serde_json::json;
use uuid::Uuid;

use crate::cmd::{CliError, CliResult};
use crate::output::{abbrev, print_output};
use crate::startup::StartupState;

/// What a rating actually did, so callers can report it honestly.
pub struct RateOutcome {
    pub feedback_id: String,
    /// Whether the feedback made it onto the gossip network. False when x0x is
    /// offline — the local row is still written either way.
    pub broadcast: bool,
}

/// Whether a resource is held locally.
///
/// `feedback_events.resource_id` carries a foreign key onto `resources(id)`, so
/// a rating can only be stored for something we actually hold. Network and name
/// results do not satisfy that until they have been propagated locally.
pub fn is_locally_indexed(conn: &rusqlite::Connection, resource_id: &str) -> bool {
    av_store::repo::resources::get(conn, resource_id)
        .ok()
        .flatten()
        .is_some()
}

/// Record a rating: store the feedback event locally and gossip it if possible.
///
/// Shared by the CLI and the web interface so both persist identical rows and
/// publish to the same topic.
pub fn record_feedback(
    conn: &rusqlite::Connection,
    state: &StartupState,
    resource_id: &str,
    rating: &str,
    query: Option<String>,
) -> CliResult<RateOutcome> {
    // Map rating string to enum
    let kind = match rating {
        "useful" => FeedbackKind::Useful,
        "not-useful" => FeedbackKind::NotUseful,
        "incorrect" => FeedbackKind::Incorrect,
        "high-confidence" => FeedbackKind::HighConfidence,
        other => return Err(CliError::Validation(format!("Invalid rating: {other}"))),
    };

    // Fail with something actionable rather than letting SQLite report a bare
    // foreign-key violation.
    if !is_locally_indexed(conn, resource_id) {
        return Err(CliError::Validation(format!(
            "cannot record feedback for {resource_id}: it is not in the local index. \
             Feedback rows reference indexed resources, so add the link with \
             'av index <uri>' first."
        )));
    }

    let agent_id = state
        .x0x_config
        .as_ref()
        .map(|c| c.agent_id.clone())
        .unwrap_or_else(|| "local".to_string());

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let event = FeedbackEvent {
        schema_version: 1,
        feedback_id: Uuid::new_v4().to_string(),
        query_text: query.clone().unwrap_or_default(),
        resource_id: resource_id.to_string(),
        by_agent_id: agent_id,
        kind,
        timestamp,
        signature: vec![], // signature placeholder
    };

    // Store feedback event locally
    av_store::repo::feedback::insert(conn, &event)
        .map_err(|e| CliError::Database(e.to_string()))?;

    // Gossip feedback if x0x is active
    let broadcast = if let Some(ref x0x_cfg) = state.x0x_config {
        let net_client = Arc::new(X0xNetClient::new(x0x_cfg.clone()));

        let payload = FeedbackPayload {
            event: event.clone(),
        };
        let envelope = build_envelope(
            &x0x_cfg.agent_id,
            MessageKind::Feedback,
            serde_json::to_value(&payload).unwrap(),
        );

        match net_client.publish(TOPIC_FEEDBACK, &envelope) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("Failed to broadcast feedback: {}", e);
                false
            }
        }
    } else {
        false
    };

    Ok(RateOutcome {
        feedback_id: event.feedback_id,
        broadcast,
    })
}

pub fn run(
    cli: crate::Cli,
    state: StartupState,
    resource_id: String,
    rating: String,
    query: Option<String>,
) -> CliResult<()> {
    let db_path = av_core::paths::db_path()
        .ok_or_else(|| CliError::Database("Failed to determine database path".to_string()))?;
    let conn = av_store::open(&db_path).map_err(|e| CliError::Database(e.to_string()))?;

    let outcome = record_feedback(&conn, &state, &resource_id, &rating, query.clone())?;
    let RateOutcome {
        feedback_id,
        broadcast,
    } = outcome;

    let output_json = json!({
        "ok": true,
        "feedback_id": feedback_id,
        "resource_id": resource_id,
        "rating": rating,
        "query": query,
        "broadcast": broadcast,
    });

    print_output(
        cli.non_interactive,
        || {
            println!(
                "Feedback recorded for resource {}: {}",
                console::style(abbrev(&resource_id, 16)).cyan(),
                console::style(&rating).green().bold()
            );
            println!("  {} Stored locally", console::style("✓").green());
            if broadcast {
                println!(
                    "  {} Broadcast to gossip network",
                    console::style("✓").green()
                );
            } else {
                println!(
                    "  {} Not broadcast (x0x offline)",
                    console::style("~").yellow()
                );
            }
        },
        &output_json,
    );

    Ok(())
}
